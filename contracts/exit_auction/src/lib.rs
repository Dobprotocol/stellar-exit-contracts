#![no_std]

use soroban_sdk::{
    contract, contractclient, contracterror, contractevent, contractimpl, contracttype, Address,
    Env,
};

const DAY_IN_LEDGERS: u32 = 17_280;
const INSTANCE_BUMP: u32 = 7 * DAY_IN_LEDGERS;
const INSTANCE_THRESHOLD: u32 = INSTANCE_BUMP - DAY_IN_LEDGERS;
const PERSISTENT_BUMP: u32 = 30 * DAY_IN_LEDGERS;
const PERSISTENT_THRESHOLD: u32 = PERSISTENT_BUMP - DAY_IN_LEDGERS;

pub const BPS_DENOMINATOR: i128 = 10_000;

/// Bidding windows are minutes, not days. An exit is a request for a price
/// right now, and a node that needs longer than this to answer is not a node
/// the seller is waiting on.
pub const DEFAULT_DURATION: u64 = 300;
pub const MIN_DURATION: u64 = 60;
pub const MAX_DURATION: u64 = 24 * 60 * 60;

/// Error codes are disjoint across the layer — lp_vault 1-99, settlement_router
/// 100-199, fifo_queue 200-299, exit_auction 300-399 — so a code that surfaces
/// through a cross-contract call still says which contract refused. A bid
/// rejected by the node's own vault terms and a bid rejected by the auction are
/// different failures and must never decode to the same thing.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 301,
    NotInitialized = 302,
    UnknownExit = 303,
    InvalidAmount = 310,
    InvalidDuration = 311,
    InvalidReference = 312,
    ExitClosed = 320,
    BiddingOver = 321,
    StillBidding = 322,
    BidTooLow = 323,
    NoBids = 324,
    ReserveNotMet = 325,
    NotAtHead = 326,
}

// ── the contracts this one drives ──

#[contractclient(name = "VaultClient")]
pub trait VaultInterface {
    fn commit(e: Env, node: Address, asset: Address, amount: i128, discount_bps: u32);
    fn release(e: Env, node: Address, asset: Address, amount: i128);
}

#[contractclient(name = "RouterClient")]
pub trait RouterInterface {
    fn escrow(e: Env, exit_id: u64, seller: Address, asset: Address, amount: i128);
    fn settle(e: Env, exit_id: u64, node: Address, token_amount: i128, usdc_amount: i128);
    fn refund(e: Env, exit_id: u64);
}

#[contractclient(name = "QueueClient")]
pub trait QueueInterface {
    fn enqueue(e: Env, asset: Address, exit_id: u64) -> u32;
    fn dequeue(e: Env, asset: Address, exit_id: u64);
    fn head(e: Env, asset: Address) -> Option<u64>;
}

// ── state ──

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[contracttype]
pub enum Status {
    /// Taking bids.
    Open = 0,
    /// Filled. Terminal.
    Settled = 1,
    /// The window closed with nothing acceptable. Still holding the seller's
    /// tokens, still taking bids, but now with a public place in line.
    Queued = 2,
    /// The seller walked away. Terminal.
    Cancelled = 3,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Config {
    pub admin: Address,
    pub vault: Address,
    pub router: Address,
    pub queue: Address,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Exit {
    pub id: u64,
    pub seller: Address,
    pub asset: Address,
    /// Participation tokens for sale. Bids are for all of it or nothing.
    pub amount: i128,
    /// What the size is worth at the seller's declared reference, in USDC.
    /// It is not a price feed and nothing is paid from it: it exists so a bid
    /// can be expressed as a discount in the events, and so nodes can see what
    /// the seller thinks the position is worth before answering.
    pub reference_usdc: i128,
    /// The seller's reserve, gross. Below this the window closes into the queue
    /// instead of filling.
    pub min_accept_usdc: i128,
    pub opened_at: u64,
    pub closes_at: u64,
    pub status: Status,
    pub best_node: Option<Address>,
    /// Gross USDC of the standing best bid. The seller receives it net of the
    /// protocol fee, which the router applies.
    pub best_usdc: i128,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Config,
    NextId,
    Exit(u64),
}

// ── events ──

/// `exit.opened` — a position is on the block and the tokens are escrowed.
#[contractevent]
pub struct Opened {
    #[topic]
    pub asset: Address,
    pub exit_id: u64,
    pub seller: Address,
    pub amount: i128,
    pub reference_usdc: i128,
    pub min_accept_usdc: i128,
    pub closes_at: u64,
}

/// `exit.bid` — a node put capital behind a price. The bid is live: the USDC is
/// committed in the vault until it is beaten, settled or released.
#[contractevent]
pub struct Bid {
    #[topic]
    pub asset: Address,
    pub exit_id: u64,
    pub node: Address,
    pub usdc_amount: i128,
    /// Derived from the seller's declared reference. Descriptive, not binding —
    /// what binds is the absolute USDC the node named.
    pub discount_bps: u32,
    pub outbid: Option<Address>,
}

/// `exit.cancelled` — the seller withdrew. Bids are conditional at fill time,
/// so nobody was owed anything.
#[contractevent]
pub struct Cancelled {
    #[topic]
    pub asset: Address,
    pub exit_id: u64,
    pub seller: Address,
}

/// # Exit Auction
///
/// A quote-driven dealer market for a position that has no order book. The
/// seller announces a size; Liquidity Nodes answer with absolute USDC amounts;
/// the best price wins. There is no matching engine, no maker side, and no
/// resting orders — every bid is backed by capital committed in the vault at
/// the moment it is placed, so a quote cannot be shown and then withdrawn.
///
/// Three rules do most of the work:
///
/// * **Price-only priority.** The highest gross bid wins, full stop. There is
///   no size preference, no fee tier, and no privileged node.
/// * **The line is respected at fill time.** While an asset has a queue, only
///   the exit at its head can settle. Bidding on the others stays open, but
///   nobody buys their way past a seller who has been waiting.
/// * **The seller is never obligated.** A bid is an offer; accepting it is a
///   separate signature. Cancelling costs the seller nothing and pays the
///   bidder nothing.
#[contract]
pub struct ExitAuction;

#[contractimpl]
impl ExitAuction {
    pub fn initialize(
        e: Env,
        admin: Address,
        vault: Address,
        router: Address,
        queue: Address,
    ) -> Result<(), Error> {
        if e.storage().instance().has(&DataKey::Config) {
            return Err(Error::AlreadyInitialized);
        }
        e.storage().instance().set(
            &DataKey::Config,
            &Config {
                admin,
                vault,
                router,
                queue,
            },
        );
        e.storage().instance().set(&DataKey::NextId, &1u64);
        Ok(())
    }

    pub fn set_admin(e: Env, new_admin: Address) -> Result<(), Error> {
        let mut config = Self::config(&e)?;
        config.admin.require_auth();
        config.admin = new_admin;
        e.storage().instance().set(&DataKey::Config, &config);
        Self::bump_instance(&e);
        Ok(())
    }

    // ========================================================================
    // The seller's side
    // ========================================================================

    /// Put a position up for exit. The tokens move into router escrow in the
    /// same transaction, so every bid that follows is against tokens that are
    /// demonstrably there.
    pub fn open_exit(
        e: Env,
        seller: Address,
        asset: Address,
        amount: i128,
        reference_usdc: i128,
        min_accept_usdc: i128,
        duration: Option<u64>,
    ) -> Result<u64, Error> {
        seller.require_auth();
        let config = Self::config(&e)?;

        if amount <= 0 || min_accept_usdc < 0 {
            return Err(Error::InvalidAmount);
        }
        if reference_usdc <= 0 {
            return Err(Error::InvalidReference);
        }
        let duration = duration.unwrap_or(DEFAULT_DURATION);
        if !(MIN_DURATION..=MAX_DURATION).contains(&duration) {
            return Err(Error::InvalidDuration);
        }

        let id: u64 = e.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        e.storage().instance().set(&DataKey::NextId, &(id + 1));

        RouterClient::new(&e, &config.router).escrow(&id, &seller, &asset, &amount);

        let now = e.ledger().timestamp();
        let exit = Exit {
            id,
            seller: seller.clone(),
            asset: asset.clone(),
            amount,
            reference_usdc,
            min_accept_usdc,
            opened_at: now,
            closes_at: now + duration,
            status: Status::Open,
            best_node: None,
            best_usdc: 0,
        };
        Self::write_exit(&e, &exit);
        Self::bump_instance(&e);

        Opened {
            asset,
            exit_id: id,
            seller,
            amount,
            reference_usdc,
            min_accept_usdc,
            closes_at: exit.closes_at,
        }
        .publish(&e);
        Ok(id)
    }

    /// Take the standing bid now, without waiting for the window to close. This
    /// is the ordinary path: the seller asked what the position is worth, saw a
    /// number, and said yes.
    pub fn accept_bid(e: Env, exit_id: u64) -> Result<(), Error> {
        let mut exit = Self::read_exit(&e, exit_id)?;
        exit.seller.require_auth();
        Self::require_live(&exit)?;
        if exit.best_node.is_none() {
            return Err(Error::NoBids);
        }
        Self::fill(&e, &mut exit)
    }

    /// Give up on the exit and take the tokens back. Allowed at any point
    /// before it fills, including from the queue.
    pub fn cancel(e: Env, exit_id: u64) -> Result<(), Error> {
        let mut exit = Self::read_exit(&e, exit_id)?;
        exit.seller.require_auth();
        Self::require_live(&exit)?;
        let config = Self::config(&e)?;

        Self::release_best(&e, &config, &exit);
        if exit.status == Status::Queued {
            QueueClient::new(&e, &config.queue).dequeue(&exit.asset, &exit.id);
        }
        RouterClient::new(&e, &config.router).refund(&exit.id);

        exit.status = Status::Cancelled;
        exit.best_node = None;
        exit.best_usdc = 0;
        Self::write_exit(&e, &exit);

        Cancelled {
            asset: exit.asset,
            exit_id,
            seller: exit.seller,
        }
        .publish(&e);
        Ok(())
    }

    // ========================================================================
    // The node's side
    // ========================================================================

    /// Bid an absolute USDC amount for the whole size.
    ///
    /// Absolute, not a discount: whatever the seller declared as a reference,
    /// and whatever any price feed says, a node can only ever be held to the
    /// number it named itself. The vault checks the money is there and that the
    /// node's own standing terms allow the fill; the previous best bid is
    /// released in the same call.
    pub fn place_bid(e: Env, node: Address, exit_id: u64, usdc_amount: i128) -> Result<(), Error> {
        node.require_auth();
        let config = Self::config(&e)?;
        let mut exit = Self::read_exit(&e, exit_id)?;
        Self::require_live(&exit)?;
        if exit.status == Status::Open && e.ledger().timestamp() > exit.closes_at {
            return Err(Error::BiddingOver);
        }
        if usdc_amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        // Price-only priority, and ties do not displace: matching the standing
        // bid is not beating it.
        if usdc_amount <= exit.best_usdc {
            return Err(Error::BidTooLow);
        }

        let discount_bps = Self::discount_bps(exit.reference_usdc, usdc_amount);
        let vault = VaultClient::new(&e, &config.vault);
        vault.commit(&node, &exit.asset, &usdc_amount, &discount_bps);

        let outbid = exit.best_node.clone();
        if let Some(prev) = outbid.clone() {
            vault.release(&prev, &exit.asset, &exit.best_usdc);
        }

        exit.best_node = Some(node.clone());
        exit.best_usdc = usdc_amount;
        Self::write_exit(&e, &exit);

        Bid {
            asset: exit.asset,
            exit_id,
            node,
            usdc_amount,
            discount_bps,
            outbid,
        }
        .publish(&e);
        Ok(())
    }

    // ========================================================================
    // Closing the window
    // ========================================================================

    /// Settle if the best bid met the seller's reserve, otherwise put the exit
    /// in line. Permissionless once the window is over — anyone can close it,
    /// because leaving capital committed to a finished auction helps nobody.
    pub fn close(e: Env, exit_id: u64) -> Result<(), Error> {
        let mut exit = Self::read_exit(&e, exit_id)?;
        if exit.status != Status::Open {
            return Err(Error::ExitClosed);
        }
        if e.ledger().timestamp() <= exit.closes_at {
            return Err(Error::StillBidding);
        }

        let acceptable = exit.best_node.is_some() && exit.best_usdc >= exit.min_accept_usdc;
        if acceptable && Self::at_head(&e, &exit)? {
            return Self::fill(&e, &mut exit);
        }

        // Nothing acceptable: the seller takes a place in line rather than
        // accepting a price they said no to. The tokens stay escrowed and bids
        // stay open — the queue is a waiting room, not a rejection.
        let config = Self::config(&e)?;
        QueueClient::new(&e, &config.queue).enqueue(&exit.asset, &exit.id);
        exit.status = Status::Queued;
        Self::write_exit(&e, &exit);
        Ok(())
    }

    // ========================================================================
    // Queries
    // ========================================================================

    pub fn get_config(e: Env) -> Result<Config, Error> {
        Self::config(&e)
    }

    pub fn get_exit(e: Env, exit_id: u64) -> Option<Exit> {
        let key = DataKey::Exit(exit_id);
        let exit: Exit = e.storage().persistent().get(&key)?;
        Some(exit)
    }

    pub fn next_id(e: Env) -> u64 {
        e.storage().instance().get(&DataKey::NextId).unwrap_or(1)
    }

    /// The discount a given gross bid represents against an exit's reference,
    /// in basis points. Quoted by the front end so the percentage on screen is
    /// the one the events will carry.
    pub fn quote_discount_bps(e: Env, exit_id: u64, usdc_amount: i128) -> Result<u32, Error> {
        let exit = Self::read_exit(&e, exit_id)?;
        if usdc_amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        Ok(Self::discount_bps(exit.reference_usdc, usdc_amount))
    }

    // ========================================================================
    // Internals
    // ========================================================================

    fn config(e: &Env) -> Result<Config, Error> {
        e.storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(Error::NotInitialized)
    }

    fn read_exit(e: &Env, exit_id: u64) -> Result<Exit, Error> {
        let key = DataKey::Exit(exit_id);
        let exit: Exit = e
            .storage()
            .persistent()
            .get(&key)
            .ok_or(Error::UnknownExit)?;
        e.storage()
            .persistent()
            .extend_ttl(&key, PERSISTENT_THRESHOLD, PERSISTENT_BUMP);
        Ok(exit)
    }

    fn write_exit(e: &Env, exit: &Exit) {
        let key = DataKey::Exit(exit.id);
        e.storage().persistent().set(&key, exit);
        e.storage()
            .persistent()
            .extend_ttl(&key, PERSISTENT_THRESHOLD, PERSISTENT_BUMP);
    }

    /// Open or Queued: still holding tokens, still able to fill.
    fn require_live(exit: &Exit) -> Result<(), Error> {
        match exit.status {
            Status::Open | Status::Queued => Ok(()),
            _ => Err(Error::ExitClosed),
        }
    }

    fn at_head(e: &Env, exit: &Exit) -> Result<bool, Error> {
        let config = Self::config(e)?;
        let head = QueueClient::new(e, &config.queue).head(&exit.asset);
        Ok(match head {
            None => true,
            Some(id) => id == exit.id,
        })
    }

    /// Hand the exit to the router. The vault commitment the winning bid made
    /// is exactly the gross being paid, so settlement consumes it whole.
    fn fill(e: &Env, exit: &mut Exit) -> Result<(), Error> {
        if exit.best_usdc < exit.min_accept_usdc {
            return Err(Error::ReserveNotMet);
        }
        if !Self::at_head(e, exit)? {
            return Err(Error::NotAtHead);
        }
        let config = Self::config(e)?;
        let node = exit.best_node.clone().ok_or(Error::NoBids)?;

        RouterClient::new(e, &config.router).settle(
            &exit.id,
            &node,
            &exit.amount,
            &exit.best_usdc,
        );

        if exit.status == Status::Queued {
            QueueClient::new(e, &config.queue).dequeue(&exit.asset, &exit.id);
        }
        exit.status = Status::Settled;
        Self::write_exit(e, exit);
        Ok(())
    }

    fn release_best(e: &Env, config: &Config, exit: &Exit) {
        if let Some(prev) = exit.best_node.clone() {
            VaultClient::new(e, &config.vault).release(&prev, &exit.asset, &exit.best_usdc);
        }
    }

    fn discount_bps(reference_usdc: i128, usdc_amount: i128) -> u32 {
        if usdc_amount >= reference_usdc {
            return 0;
        }
        ((reference_usdc - usdc_amount) * BPS_DENOMINATOR / reference_usdc) as u32
    }

    fn bump_instance(e: &Env) {
        e.storage()
            .instance()
            .extend_ttl(INSTANCE_THRESHOLD, INSTANCE_BUMP);
    }
}

#[cfg(test)]
mod tests;
