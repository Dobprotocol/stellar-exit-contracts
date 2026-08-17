use soroban_sdk::{contract, contractimpl, token, Address, Env};

use crate::errors::Error;
use crate::events;
use crate::storage::{
    self, Appetite, Config, Node, WithdrawRequest, BPS_DENOMINATOR, DEFAULT_MAX_SINGLE_FILL_BPS,
    DEFAULT_WITHDRAW_TIMELOCK, MIN_BACKING_AGE,
};

/// # LP Liquidity Vault
///
/// Where Liquidity Nodes park the USDC they use to buy exits, and where they
/// declare which assets they will back and on what terms.
///
/// Two limits live here and they are **not** the same kind of limit:
///
/// * **Free balance** (`deposited − committed − pending_withdrawal`) is the hard
///   solvency invariant. It is real USDC held by this contract and it is what
///   guarantees a winning bid can actually be paid. Nothing can bypass it.
/// * **Exposure** (`Appetite.exposure` against `max_exposure`) is a soft, self-imposed
///   risk limit. It keeps counting after a payout because the node is then holding the
///   asset, and only the node itself can mark a position as divested via
///   [`reduce_exposure`]. A node lying to itself here risks only its own capital;
///   it can never make the vault insolvent.
///
/// Pricing is deliberately absent. The node names a floor discount and a ceiling
/// exposure; *why* those numbers — the TRUFA score of the asset, the operator's own
/// model, whatever it reads off the primary market — is decided off-chain.
#[contract]
pub struct LpVault;

#[contractimpl]
impl LpVault {
    // ========================================================================
    // Lifecycle
    // ========================================================================

    pub fn initialize(
        e: Env,
        admin: Address,
        usdc: Address,
        min_deposit: i128,
        max_single_fill_bps: Option<u32>,
        withdraw_timelock: Option<u64>,
    ) -> Result<(), Error> {
        if storage::has_config(&e) {
            return Err(Error::AlreadyInitialized);
        }
        if min_deposit < 0 {
            return Err(Error::InvalidAmount);
        }
        let cap = max_single_fill_bps.unwrap_or(DEFAULT_MAX_SINGLE_FILL_BPS);
        if cap == 0 || cap as i128 > BPS_DENOMINATOR {
            return Err(Error::InvalidBps);
        }

        storage::write_config(
            &e,
            &Config {
                admin,
                usdc,
                auction: None,
                router: None,
                min_deposit,
                max_single_fill_bps: cap,
                withdraw_timelock: withdraw_timelock.unwrap_or(DEFAULT_WITHDRAW_TIMELOCK),
            },
        );
        Ok(())
    }

    /// Wire the exit_auction. Only it may commit and release capital.
    pub fn set_auction(e: Env, auction: Address) -> Result<(), Error> {
        let mut config = storage::read_config(&e)?;
        config.admin.require_auth();
        config.auction = Some(auction);
        storage::write_config(&e, &config);
        Ok(())
    }

    /// Wire the settlement_router. Only it may move capital out to a seller.
    pub fn set_router(e: Env, router: Address) -> Result<(), Error> {
        let mut config = storage::read_config(&e)?;
        config.admin.require_auth();
        config.router = Some(router);
        storage::write_config(&e, &config);
        Ok(())
    }

    pub fn set_admin(e: Env, new_admin: Address) -> Result<(), Error> {
        let mut config = storage::read_config(&e)?;
        config.admin.require_auth();
        config.admin = new_admin;
        storage::write_config(&e, &config);
        Ok(())
    }

    pub fn set_max_single_fill_bps(e: Env, bps: u32) -> Result<(), Error> {
        let mut config = storage::read_config(&e)?;
        config.admin.require_auth();
        if bps == 0 || bps as i128 > BPS_DENOMINATOR {
            return Err(Error::InvalidBps);
        }
        config.max_single_fill_bps = bps;
        storage::write_config(&e, &config);
        Ok(())
    }

    // ========================================================================
    // Node capital
    // ========================================================================

    pub fn deposit(e: Env, node: Address, amount: i128) -> Result<(), Error> {
        node.require_auth();
        let config = storage::read_config(&e)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let mut state = storage::read_node(&e, &node).unwrap_or(Node {
            deposited: 0,
            committed: 0,
            filled: 0,
            joined_at: e.ledger().timestamp(),
        });

        if state.deposited == 0 && amount < config.min_deposit {
            return Err(Error::BelowMinDeposit);
        }

        token::Client::new(&e, &config.usdc).transfer(
            &node,
            &e.current_contract_address(),
            &amount,
        );

        state.deposited += amount;
        storage::write_node(&e, &node, &state);

        events::Deposit {
            node,
            amount,
            total_deposited: state.deposited,
        }
        .publish(&e);
        Ok(())
    }

    /// Start the timelock on taking capital back out. The amount is reserved
    /// immediately so it cannot be committed to a bid in the meantime.
    pub fn request_withdrawal(e: Env, node: Address, amount: i128) -> Result<(), Error> {
        node.require_auth();
        let config = storage::read_config(&e)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        if storage::read_withdrawal(&e, &node).is_some() {
            return Err(Error::WithdrawalPending);
        }
        let state = storage::read_node(&e, &node).ok_or(Error::UnknownNode)?;
        if state.deposited - state.committed < amount {
            return Err(Error::InsufficientFreeBalance);
        }

        let unlock_at = e.ledger().timestamp() + config.withdraw_timelock;
        storage::write_withdrawal(&e, &node, &WithdrawRequest { amount, unlock_at });

        events::WithdrawRequested {
            node,
            amount,
            unlock_at,
        }
        .publish(&e);
        Ok(())
    }

    pub fn cancel_withdrawal(e: Env, node: Address) -> Result<(), Error> {
        node.require_auth();
        storage::read_config(&e)?;
        let req = storage::read_withdrawal(&e, &node).ok_or(Error::NoWithdrawalPending)?;
        storage::clear_withdrawal(&e, &node);

        events::WithdrawCancelled {
            node,
            amount: req.amount,
        }
        .publish(&e);
        Ok(())
    }

    pub fn execute_withdrawal(e: Env, node: Address) -> Result<(), Error> {
        node.require_auth();
        let config = storage::read_config(&e)?;
        let req = storage::read_withdrawal(&e, &node).ok_or(Error::NoWithdrawalPending)?;
        if e.ledger().timestamp() < req.unlock_at {
            return Err(Error::TimelockNotElapsed);
        }

        let mut state = storage::read_node(&e, &node).ok_or(Error::UnknownNode)?;
        // Re-check against live state: capital may have been committed since the request.
        if state.deposited - state.committed < req.amount {
            return Err(Error::InsufficientFreeBalance);
        }

        state.deposited -= req.amount;
        storage::write_node(&e, &node, &state);
        storage::clear_withdrawal(&e, &node);

        token::Client::new(&e, &config.usdc).transfer(
            &e.current_contract_address(),
            &node,
            &req.amount,
        );

        events::Withdraw {
            node,
            amount: req.amount,
            total_deposited: state.deposited,
        }
        .publish(&e);
        Ok(())
    }

    // ========================================================================
    // Appetite — the node's standing terms for one asset
    // ========================================================================

    pub fn set_appetite(
        e: Env,
        node: Address,
        asset: Address,
        max_exposure: i128,
        min_discount_bps: u32,
        active: bool,
    ) -> Result<(), Error> {
        node.require_auth();
        storage::read_config(&e)?;
        if max_exposure < 0 {
            return Err(Error::InvalidAmount);
        }
        if min_discount_bps as i128 >= BPS_DENOMINATOR {
            return Err(Error::InvalidBps);
        }
        storage::read_node(&e, &node).ok_or(Error::UnknownNode)?;

        let existing = storage::read_appetite(&e, &node, &asset);
        // Re-arming a backing restarts MIN_BACKING_AGE; raising the ceiling on a
        // live one does not, so a node is not punished for adding capacity.
        let (exposure, backed_at) = match &existing {
            Some(prev) if prev.active && active => (prev.exposure, prev.backed_at),
            Some(prev) => (prev.exposure, e.ledger().timestamp()),
            None => (0, e.ledger().timestamp()),
        };

        storage::write_appetite(
            &e,
            &node,
            &asset,
            &Appetite {
                max_exposure,
                exposure,
                min_discount_bps,
                active,
                backed_at,
            },
        );

        events::AppetiteSet {
            node,
            asset,
            max_exposure,
            min_discount_bps,
            active,
        }
        .publish(&e);
        Ok(())
    }

    /// Mark part of a position as divested, freeing headroom under `max_exposure`.
    ///
    /// Self-reported on purpose: once the node holds the asset it can sell it
    /// anywhere — the primary market included — and this contract has no way to
    /// observe that. This moves a soft limit only; it can never touch free balance.
    pub fn reduce_exposure(
        e: Env,
        node: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), Error> {
        node.require_auth();
        storage::read_config(&e)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let mut appetite = storage::read_appetite(&e, &node, &asset).ok_or(Error::NoAppetite)?;
        appetite.exposure = if amount > appetite.exposure {
            0
        } else {
            appetite.exposure - amount
        };
        storage::write_appetite(&e, &node, &asset, &appetite);
        Ok(())
    }

    // ========================================================================
    // Auction hooks — lock and unlock capital behind live bids
    // ========================================================================

    /// Lock `amount` USDC behind a bid. Callable only by the wired exit_auction.
    ///
    /// `discount_bps` is the bid the auction is about to record; it is checked
    /// against the node's own floor here so the floor cannot be bypassed by a
    /// bug in the auction.
    pub fn commit(
        e: Env,
        node: Address,
        asset: Address,
        amount: i128,
        discount_bps: u32,
    ) -> Result<(), Error> {
        let config = storage::read_config(&e)?;
        let auction = config.auction.clone().ok_or(Error::NotWired)?;
        auction.require_auth();

        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let mut state = storage::read_node(&e, &node).ok_or(Error::UnknownNode)?;
        let mut appetite = storage::read_appetite(&e, &node, &asset).ok_or(Error::NoAppetite)?;

        if !appetite.active {
            return Err(Error::AppetiteInactive);
        }
        if e.ledger().timestamp() - appetite.backed_at < MIN_BACKING_AGE {
            return Err(Error::BackingTooYoung);
        }
        if discount_bps < appetite.min_discount_bps {
            return Err(Error::DiscountBelowFloor);
        }

        let reserved = storage::read_withdrawal(&e, &node)
            .map(|w| w.amount)
            .unwrap_or(0);
        if state.deposited - state.committed - reserved < amount {
            return Err(Error::InsufficientFreeBalance);
        }
        if appetite.exposure + amount > appetite.max_exposure {
            return Err(Error::ExposureExceeded);
        }
        let single_fill_cap =
            state.deposited * config.max_single_fill_bps as i128 / BPS_DENOMINATOR;
        if amount > single_fill_cap {
            return Err(Error::SingleFillCapExceeded);
        }

        state.committed += amount;
        appetite.exposure += amount;
        storage::write_node(&e, &node, &state);
        storage::write_appetite(&e, &node, &asset, &appetite);

        events::Committed {
            node,
            asset,
            amount,
        }
        .publish(&e);
        Ok(())
    }

    /// Unlock capital behind a bid that did not win. Callable only by exit_auction.
    pub fn release(e: Env, node: Address, asset: Address, amount: i128) -> Result<(), Error> {
        let config = storage::read_config(&e)?;
        let auction = config.auction.clone().ok_or(Error::NotWired)?;
        auction.require_auth();

        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let mut state = storage::read_node(&e, &node).ok_or(Error::UnknownNode)?;
        let mut appetite = storage::read_appetite(&e, &node, &asset).ok_or(Error::NoAppetite)?;
        if state.committed < amount || appetite.exposure < amount {
            return Err(Error::CommitUnderflow);
        }

        state.committed -= amount;
        appetite.exposure -= amount;
        storage::write_node(&e, &node, &state);
        storage::write_appetite(&e, &node, &asset, &appetite);

        events::Released {
            node,
            asset,
            amount,
        }
        .publish(&e);
        Ok(())
    }

    // ========================================================================
    // Router hook — the only way USDC leaves for a seller
    // ========================================================================

    /// Pay a settled exit. Callable only by the wired settlement_router.
    ///
    /// Consumes the commitment rather than releasing it: the USDC leaves and the
    /// node's exposure to the asset stays on the books, because the node now
    /// holds the asset.
    pub fn pay_out(
        e: Env,
        node: Address,
        asset: Address,
        to: Address,
        amount: i128,
    ) -> Result<(), Error> {
        let config = storage::read_config(&e)?;
        let router = config.router.clone().ok_or(Error::NotWired)?;
        router.require_auth();

        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let mut state = storage::read_node(&e, &node).ok_or(Error::UnknownNode)?;
        storage::read_appetite(&e, &node, &asset).ok_or(Error::NoAppetite)?;
        if state.committed < amount || state.deposited < amount {
            return Err(Error::CommitUnderflow);
        }

        state.committed -= amount;
        state.deposited -= amount;
        state.filled += amount;
        storage::write_node(&e, &node, &state);

        token::Client::new(&e, &config.usdc).transfer(&e.current_contract_address(), &to, &amount);

        events::PaidOut {
            node,
            asset,
            to,
            amount,
        }
        .publish(&e);
        Ok(())
    }

    // ========================================================================
    // Queries
    // ========================================================================

    pub fn get_config(e: Env) -> Result<Config, Error> {
        storage::read_config(&e)
    }

    pub fn get_node(e: Env, node: Address) -> Result<Node, Error> {
        storage::read_node(&e, &node).ok_or(Error::UnknownNode)
    }

    pub fn get_appetite(e: Env, node: Address, asset: Address) -> Result<Appetite, Error> {
        storage::read_appetite(&e, &node, &asset).ok_or(Error::NoAppetite)
    }

    pub fn get_withdrawal(e: Env, node: Address) -> Option<WithdrawRequest> {
        storage::read_withdrawal(&e, &node)
    }

    /// USDC that could still be committed: deposited − committed − pending withdrawal.
    pub fn free_balance(e: Env, node: Address) -> i128 {
        let state = match storage::read_node(&e, &node) {
            Some(s) => s,
            None => return 0,
        };
        let reserved = storage::read_withdrawal(&e, &node)
            .map(|w| w.amount)
            .unwrap_or(0);
        let free = state.deposited - state.committed - reserved;
        if free < 0 {
            0
        } else {
            free
        }
    }

    /// The largest USDC amount this node could commit to one exit on `asset`
    /// right now at `discount_bps`. Zero means it will not fill — the whole
    /// off-chain book can be built from this one call per node.
    pub fn quote_capacity(e: Env, node: Address, asset: Address, discount_bps: u32) -> i128 {
        let config = match storage::read_config(&e) {
            Ok(c) => c,
            Err(_) => return 0,
        };
        let state = match storage::read_node(&e, &node) {
            Some(s) => s,
            None => return 0,
        };
        let appetite = match storage::read_appetite(&e, &node, &asset) {
            Some(a) => a,
            None => return 0,
        };
        if !appetite.active
            || discount_bps < appetite.min_discount_bps
            || e.ledger().timestamp() - appetite.backed_at < MIN_BACKING_AGE
        {
            return 0;
        }

        let reserved = storage::read_withdrawal(&e, &node)
            .map(|w| w.amount)
            .unwrap_or(0);
        let mut capacity = state.deposited - state.committed - reserved;

        let headroom = appetite.max_exposure - appetite.exposure;
        if headroom < capacity {
            capacity = headroom;
        }
        let single_fill_cap =
            state.deposited * config.max_single_fill_bps as i128 / BPS_DENOMINATOR;
        if single_fill_cap < capacity {
            capacity = single_fill_cap;
        }

        if capacity < 0 {
            0
        } else {
            capacity
        }
    }
}
