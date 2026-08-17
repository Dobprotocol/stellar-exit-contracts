#![no_std]
//! # Testnet faucet
//!
//! **This is not part of the exit layer.** It exists because the test tokens on
//! testnet mint only for their admin, so someone opening the app with an empty
//! wallet has nothing to sell and nothing to bid with. The faucet becomes the
//! admin of those tokens and hands out a fixed drip, once per cooldown, to
//! whoever asks and signs.
//!
//! Nothing here is meant for mainnet: it is a contract whose whole purpose is
//! to give away money. It lives outside `contracts/` so that it is never
//! mistaken for one of the four, and its error codes sit at 900-999, well clear
//! of the layer's ranges.

use soroban_sdk::{
    contract, contractclient, contracterror, contractevent, contractimpl, contracttype, Address,
    Env, Vec,
};

const DAY_IN_LEDGERS: u32 = 17_280;
const BUMP_AMOUNT: u32 = 30 * DAY_IN_LEDGERS;
const LIFETIME_THRESHOLD: u32 = BUMP_AMOUNT - DAY_IN_LEDGERS;

/// A tap that refills faster than a person can spend is a tap that drains the
/// ledger with a loop, so a claim is rate-limited per (claimant, token).
pub const DEFAULT_COOLDOWN: u64 = 3_600;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 901,
    NotInitialized = 902,
    UnknownToken = 903,
    TooSoon = 904,
    InvalidAmount = 905,
}

/// The token interface the faucet drives. `mint` is admin-gated inside the
/// token, which is exactly why the faucet has to *be* the admin — and why
/// `set_token_admin` exists, so handing that authority back does not require
/// redeploying the token.
#[contractclient(name = "TokenClient")]
pub trait TokenInterface {
    fn mint(e: Env, to: Address, amount: i128);
    fn set_admin(e: Env, new_admin: Address);
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Config {
    pub admin: Address,
    pub cooldown: u64,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Config,
    /// token -> how much one claim hands over, in the token's own units
    Drip(Address),
    /// every token with a drip set, so the app can list the tap without
    /// knowing the addresses in advance
    Tokens,
    /// (claimant, token) -> ledger timestamp of their last claim
    Last(Address, Address),
}

#[contractevent]
pub struct Claimed {
    #[topic]
    pub token: Address,
    #[topic]
    pub to: Address,
    pub amount: i128,
    pub next_at: u64,
}

#[contractevent]
pub struct DripSet {
    #[topic]
    pub token: Address,
    pub amount: i128,
}

#[contract]
pub struct TestnetFaucet;

#[contractimpl]
impl TestnetFaucet {
    pub fn initialize(e: Env, admin: Address, cooldown: u64) -> Result<(), Error> {
        if e.storage().instance().has(&DataKey::Config) {
            return Err(Error::AlreadyInitialized);
        }
        e.storage()
            .instance()
            .set(&DataKey::Config, &Config { admin, cooldown });
        e.storage().instance().set(&DataKey::Tokens, &Vec::<Address>::new(&e));
        Ok(())
    }

    /// Register a token and how much one claim hands over. Setting a drip of 0
    /// takes the token off the tap without forgetting anyone's cooldown.
    pub fn set_drip(e: Env, token: Address, amount: i128) -> Result<(), Error> {
        let config = Self::config(&e)?;
        config.admin.require_auth();
        if amount < 0 {
            return Err(Error::InvalidAmount);
        }

        e.storage().persistent().set(&DataKey::Drip(token.clone()), &amount);
        e.storage().persistent().extend_ttl(
            &DataKey::Drip(token.clone()),
            LIFETIME_THRESHOLD,
            BUMP_AMOUNT,
        );

        let mut tokens = Self::tokens_of(&e);
        if !tokens.iter().any(|t| t == token) {
            tokens.push_back(token.clone());
            e.storage().instance().set(&DataKey::Tokens, &tokens);
        }

        DripSet { token, amount }.publish(&e);
        Ok(())
    }

    pub fn set_cooldown(e: Env, cooldown: u64) -> Result<(), Error> {
        let mut config = Self::config(&e)?;
        config.admin.require_auth();
        config.cooldown = cooldown;
        e.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    /// Hand the token's admin rights on to someone else — the escape hatch for
    /// a faucet that has to be replaced, since the token itself can only ever
    /// be initialized once.
    pub fn set_token_admin(e: Env, token: Address, new_admin: Address) -> Result<(), Error> {
        let config = Self::config(&e)?;
        config.admin.require_auth();
        TokenClient::new(&e, &token).set_admin(&new_admin);
        Ok(())
    }

    /// Take one drip. Anyone may call it for themselves; `to` signs, so the
    /// cooldown cannot be spent on someone else's behalf.
    pub fn claim(e: Env, to: Address, token: Address) -> Result<i128, Error> {
        let config = Self::config(&e)?;
        to.require_auth();

        let amount = Self::drip_of(&e, &token);
        if amount <= 0 {
            return Err(Error::UnknownToken);
        }

        let now = e.ledger().timestamp();
        let key = DataKey::Last(to.clone(), token.clone());
        let last: u64 = e.storage().persistent().get(&key).unwrap_or(0);
        // `last == 0` is "never claimed", not "claimed at the epoch": a first
        // claim must not be gated by a cooldown it was never subject to.
        if last != 0 && now < last.saturating_add(config.cooldown) {
            return Err(Error::TooSoon);
        }

        e.storage().persistent().set(&key, &now);
        e.storage()
            .persistent()
            .extend_ttl(&key, LIFETIME_THRESHOLD, BUMP_AMOUNT);

        TokenClient::new(&e, &token).mint(&to, &amount);

        Claimed {
            token,
            to,
            amount,
            next_at: now.saturating_add(config.cooldown),
        }
        .publish(&e);
        Ok(amount)
    }

    // ── queries ──

    pub fn drip(e: Env, token: Address) -> i128 {
        Self::drip_of(&e, &token)
    }

    pub fn tokens(e: Env) -> Vec<Address> {
        Self::tokens_of(&e)
    }

    /// When `who` may claim `token` again, as a ledger timestamp. 0 means now.
    pub fn next_claim(e: Env, who: Address, token: Address) -> Result<u64, Error> {
        let config = Self::config(&e)?;
        let last: u64 = e
            .storage()
            .persistent()
            .get(&DataKey::Last(who, token))
            .unwrap_or(0);
        if last == 0 {
            return Ok(0);
        }
        let next = last.saturating_add(config.cooldown);
        Ok(if e.ledger().timestamp() >= next { 0 } else { next })
    }

    pub fn get_config(e: Env) -> Result<Config, Error> {
        Self::config(&e)
    }

    // ── internals ──

    fn config(e: &Env) -> Result<Config, Error> {
        e.storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(Error::NotInitialized)
    }

    fn drip_of(e: &Env, token: &Address) -> i128 {
        e.storage()
            .persistent()
            .get(&DataKey::Drip(token.clone()))
            .unwrap_or(0)
    }

    fn tokens_of(e: &Env) -> Vec<Address> {
        e.storage()
            .instance()
            .get(&DataKey::Tokens)
            .unwrap_or_else(|| Vec::new(e))
    }
}

#[cfg(test)]
mod tests;
