#![no_std]

use soroban_sdk::{
    contract, contractclient, contracterror, contractevent, contractimpl, contracttype, token,
    Address, Env,
};

const DAY_IN_LEDGERS: u32 = 17_280;
const INSTANCE_BUMP: u32 = 7 * DAY_IN_LEDGERS;
const INSTANCE_THRESHOLD: u32 = INSTANCE_BUMP - DAY_IN_LEDGERS;
const PERSISTENT_BUMP: u32 = 30 * DAY_IN_LEDGERS;
const PERSISTENT_THRESHOLD: u32 = PERSISTENT_BUMP - DAY_IN_LEDGERS;

pub const BPS_DENOMINATOR: i128 = 10_000;

/// A ceiling the admin cannot raise past. The protocol fee is a fee, not a
/// lever for confiscating a settlement after the fact.
pub const MAX_PROTOCOL_FEE_BPS: u32 = 500;

/// Error codes are disjoint across the layer — lp_vault 1-99, settlement_router
/// 100-199, fifo_queue 200-299, exit_auction 300-399 — so a code that surfaces
/// through a cross-contract call still says which contract refused, instead of
/// being decoded as the caller's own error with the same number.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 101,
    NotInitialized = 102,
    NotAuction = 103,
    NotWired = 104,
    InvalidAmount = 110,
    InvalidBps = 111,
    AlreadyEscrowed = 112,
    NothingEscrowed = 113,
    ExceedsEscrow = 114,
}

/// The only thing the router needs from the vault: move a node's committed
/// USDC to a payee. Declared rather than imported, so the vault's code is not
/// compiled into this contract's wasm.
#[contractclient(name = "VaultClient")]
pub trait VaultInterface {
    fn pay_out(e: Env, node: Address, asset: Address, to: Address, amount: i128);
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Config {
    pub admin: Address,
    /// The lp_vault holding Liquidity Node capital.
    pub vault: Address,
    /// The cash leg. Every bid and every payout is denominated in it.
    pub usdc: Address,
    /// Where the protocol fee lands.
    pub treasury: Address,
    /// The exit_auction. Only it may escrow, settle or refund.
    pub auction: Option<Address>,
    pub protocol_fee_bps: u32,
}

/// Participation tokens the router is holding on behalf of one exit. While this
/// record exists, the tokens are off the seller's balance and unspendable by
/// anyone: they can only leave towards a node that paid for them, or back to
/// the seller.
#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Escrow {
    pub seller: Address,
    pub asset: Address,
    pub amount: i128,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Config,
    Escrow(u64), // exit_id -> Escrow
}

/// The seller's tokens are locked. From here the exit is real: the seller can
/// no longer sell them elsewhere while bids are live.
#[contractevent]
pub struct Escrowed {
    #[topic]
    pub asset: Address,
    pub exit_id: u64,
    pub seller: Address,
    pub amount: i128,
}

/// `exit.settled` — one transaction moved everything. The node received the
/// participation tokens, the seller received USDC net of the discount it
/// accepted and the protocol fee, and the treasury received the fee.
#[contractevent]
pub struct Settled {
    #[topic]
    pub asset: Address,
    pub exit_id: u64,
    pub seller: Address,
    pub node: Address,
    pub token_amount: i128,
    pub usdc_gross: i128,
    pub protocol_fee: i128,
    pub usdc_net: i128,
    /// Escrow left over — non-zero when the book only covered part of the size.
    pub remaining: i128,
}

/// The exit ended without a fill. Everything still escrowed went home.
#[contractevent]
pub struct Refunded {
    #[topic]
    pub asset: Address,
    pub exit_id: u64,
    pub seller: Address,
    pub amount: i128,
}

/// # Settlement Router
///
/// The only contract in the exit layer that moves value. The auction decides
/// *who* fills and *at what price*; the vault decides *whether a node's capital
/// is available*; this contract performs the transfer, and it performs both
/// legs or neither.
///
/// Two properties the router guarantees on its own, without trusting the
/// auction's arithmetic:
///
/// * **Nothing is paid for tokens that were never delivered.** A settlement can
///   only draw against a live escrow record, and only up to what it holds. A
///   double settle is not a policy decision made upstream — the second call
///   finds an empty escrow.
/// * **Escrowed tokens have exactly two exits.** To a node that paid for them,
///   or back to the seller. There is no admin path to them, at any fee.
#[contract]
pub struct SettlementRouter;

#[contractimpl]
impl SettlementRouter {
    pub fn initialize(
        e: Env,
        admin: Address,
        vault: Address,
        usdc: Address,
        treasury: Address,
        protocol_fee_bps: u32,
    ) -> Result<(), Error> {
        if e.storage().instance().has(&DataKey::Config) {
            return Err(Error::AlreadyInitialized);
        }
        if protocol_fee_bps > MAX_PROTOCOL_FEE_BPS {
            return Err(Error::InvalidBps);
        }
        e.storage().instance().set(
            &DataKey::Config,
            &Config {
                admin,
                vault,
                usdc,
                treasury,
                auction: None,
                protocol_fee_bps,
            },
        );
        Ok(())
    }

    // ── wiring ──

    pub fn set_auction(e: Env, auction: Address) -> Result<(), Error> {
        let mut config = Self::config(&e)?;
        config.admin.require_auth();
        config.auction = Some(auction);
        Self::write_config(&e, &config);
        Ok(())
    }

    pub fn set_admin(e: Env, new_admin: Address) -> Result<(), Error> {
        let mut config = Self::config(&e)?;
        config.admin.require_auth();
        config.admin = new_admin;
        Self::write_config(&e, &config);
        Ok(())
    }

    pub fn set_treasury(e: Env, treasury: Address) -> Result<(), Error> {
        let mut config = Self::config(&e)?;
        config.admin.require_auth();
        config.treasury = treasury;
        Self::write_config(&e, &config);
        Ok(())
    }

    pub fn set_protocol_fee_bps(e: Env, bps: u32) -> Result<(), Error> {
        let mut config = Self::config(&e)?;
        config.admin.require_auth();
        if bps > MAX_PROTOCOL_FEE_BPS {
            return Err(Error::InvalidBps);
        }
        config.protocol_fee_bps = bps;
        Self::write_config(&e, &config);
        Ok(())
    }

    // ── the exit ──

    /// Lock the seller's participation tokens for the duration of the exit.
    /// Called when the auction opens, so every bid is against tokens that are
    /// demonstrably there.
    pub fn escrow(
        e: Env,
        exit_id: u64,
        seller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), Error> {
        Self::require_auction(&e)?;
        // The auction decides that an exit opens; only the seller can hand over
        // the tokens. Both signatures are on the transaction, and the auction
        // alone can never move someone's position.
        seller.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        if e.storage().persistent().has(&DataKey::Escrow(exit_id)) {
            return Err(Error::AlreadyEscrowed);
        }

        token::Client::new(&e, &asset).transfer(
            &seller,
            &e.current_contract_address(),
            &amount,
        );

        Self::write_escrow(
            &e,
            exit_id,
            &Escrow {
                seller: seller.clone(),
                asset: asset.clone(),
                amount,
            },
        );
        Self::bump_instance(&e);

        Escrowed {
            asset,
            exit_id,
            seller,
            amount,
        }
        .publish(&e);
        Ok(())
    }

    /// Fill `token_amount` of the exit against one node at `usdc_amount`.
    ///
    /// `usdc_amount` is the gross the node bid for this slice. The seller
    /// receives it net of the protocol fee; the discount is already inside the
    /// number, because the node named it. Called more than once for the same
    /// exit when the book covered the size across several nodes.
    pub fn settle(
        e: Env,
        exit_id: u64,
        node: Address,
        token_amount: i128,
        usdc_amount: i128,
    ) -> Result<(), Error> {
        let config = Self::require_auction(&e)?;
        if token_amount <= 0 || usdc_amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let mut escrow = Self::read_escrow(&e, exit_id).ok_or(Error::NothingEscrowed)?;
        if token_amount > escrow.amount {
            return Err(Error::ExceedsEscrow);
        }

        let protocol_fee = usdc_amount * config.protocol_fee_bps as i128 / BPS_DENOMINATOR;
        let usdc_net = usdc_amount - protocol_fee;

        // Cash leg first: the node's committed capital leaves the vault towards
        // the seller and the treasury. If either payout fails — the commitment
        // is gone, the node is short — the whole call reverts and the seller
        // still owns the tokens.
        let vault = VaultClient::new(&e, &config.vault);
        vault.pay_out(&node, &escrow.asset, &escrow.seller, &usdc_net);
        if protocol_fee > 0 {
            vault.pay_out(&node, &escrow.asset, &config.treasury, &protocol_fee);
        }

        // Asset leg.
        token::Client::new(&e, &escrow.asset).transfer(
            &e.current_contract_address(),
            &node,
            &token_amount,
        );

        escrow.amount -= token_amount;
        let remaining = escrow.amount;
        if remaining == 0 {
            e.storage().persistent().remove(&DataKey::Escrow(exit_id));
        } else {
            Self::write_escrow(&e, exit_id, &escrow);
        }

        Settled {
            asset: escrow.asset,
            exit_id,
            seller: escrow.seller,
            node,
            token_amount,
            usdc_gross: usdc_amount,
            protocol_fee,
            usdc_net,
            remaining,
        }
        .publish(&e);
        Ok(())
    }

    /// Return whatever is still escrowed to the seller: the exit was cancelled,
    /// or it went to the queue and the seller is not going to leave tokens
    /// locked while it waits.
    pub fn refund(e: Env, exit_id: u64) -> Result<(), Error> {
        Self::require_auction(&e)?;

        let escrow = Self::read_escrow(&e, exit_id).ok_or(Error::NothingEscrowed)?;
        e.storage().persistent().remove(&DataKey::Escrow(exit_id));

        token::Client::new(&e, &escrow.asset).transfer(
            &e.current_contract_address(),
            &escrow.seller,
            &escrow.amount,
        );

        Refunded {
            asset: escrow.asset,
            exit_id,
            seller: escrow.seller,
            amount: escrow.amount,
        }
        .publish(&e);
        Ok(())
    }

    // ── queries ──

    pub fn get_config(e: Env) -> Result<Config, Error> {
        Self::config(&e)
    }

    pub fn get_escrow(e: Env, exit_id: u64) -> Option<Escrow> {
        Self::read_escrow(&e, exit_id)
    }

    /// What a gross bid is worth to the seller. The front end quotes from this
    /// so the number on screen is the number the contract computes.
    pub fn quote_net(e: Env, usdc_amount: i128) -> Result<i128, Error> {
        let config = Self::config(&e)?;
        if usdc_amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        Ok(usdc_amount - usdc_amount * config.protocol_fee_bps as i128 / BPS_DENOMINATOR)
    }

    // ── internals ──

    fn config(e: &Env) -> Result<Config, Error> {
        e.storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(Error::NotInitialized)
    }

    fn write_config(e: &Env, config: &Config) {
        e.storage().instance().set(&DataKey::Config, config);
        Self::bump_instance(e);
    }

    fn require_auction(e: &Env) -> Result<Config, Error> {
        let config = Self::config(e)?;
        let auction = config.auction.clone().ok_or(Error::NotWired)?;
        auction.require_auth();
        Ok(config)
    }

    fn read_escrow(e: &Env, exit_id: u64) -> Option<Escrow> {
        let key = DataKey::Escrow(exit_id);
        let escrow: Escrow = e.storage().persistent().get(&key)?;
        e.storage()
            .persistent()
            .extend_ttl(&key, PERSISTENT_THRESHOLD, PERSISTENT_BUMP);
        Some(escrow)
    }

    fn write_escrow(e: &Env, exit_id: u64, escrow: &Escrow) {
        let key = DataKey::Escrow(exit_id);
        e.storage().persistent().set(&key, escrow);
        e.storage()
            .persistent()
            .extend_ttl(&key, PERSISTENT_THRESHOLD, PERSISTENT_BUMP);
    }

    fn bump_instance(e: &Env) {
        e.storage()
            .instance()
            .extend_ttl(INSTANCE_THRESHOLD, INSTANCE_BUMP);
    }
}

#[cfg(test)]
mod tests;
