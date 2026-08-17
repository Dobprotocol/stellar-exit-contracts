#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, Address, Env, Vec,
};

const DAY_IN_LEDGERS: u32 = 17_280;
const BUMP_AMOUNT: u32 = 30 * DAY_IN_LEDGERS;
const LIFETIME_THRESHOLD: u32 = BUMP_AMOUNT - DAY_IN_LEDGERS;

/// A queue is a stored `Vec`, so it has to be bounded. Past this depth the
/// asset is simply out of road and the auction says so, rather than accepting
/// an exit it cannot account for.
pub const MAX_QUEUE_DEPTH: u32 = 200;

/// Error codes are disjoint across the layer — lp_vault 1-99, settlement_router
/// 100-199, fifo_queue 200-299, exit_auction 300-399 — so a code that surfaces
/// through a cross-contract call still says which contract refused, instead of
/// being decoded as the caller's own error with the same number.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 201,
    NotInitialized = 202,
    NotAuction = 203,
    AlreadyQueued = 204,
    NotQueued = 205,
    QueueFull = 206,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Config {
    pub admin: Address,
    /// The exit_auction. Only it may move exits in and out of the line.
    pub auction: Option<Address>,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Config,
    Queue(Address), // asset -> ordered exit ids
}

/// `exit.queued` — no acceptable bid; the exit took a public position in line.
#[contractevent]
pub struct Queued {
    #[topic]
    pub asset: Address,
    pub exit_id: u64,
    pub position: u32,
    pub depth: u32,
}

/// The exit left the line, because it settled or the seller withdrew it.
#[contractevent]
pub struct Dequeued {
    #[topic]
    pub asset: Address,
    pub exit_id: u64,
    pub depth: u32,
}

/// # FIFO Queue Manager
///
/// When demand for the door exceeds the capital standing behind it, exits wait
/// instead of racing. Position is public, leaving is always allowed, and there
/// is no function anywhere that reorders the line — not for the admin either.
#[contract]
pub struct FifoQueue;

#[contractimpl]
impl FifoQueue {
    pub fn initialize(e: Env, admin: Address) -> Result<(), Error> {
        if e.storage().instance().has(&DataKey::Config) {
            return Err(Error::AlreadyInitialized);
        }
        e.storage().instance().set(
            &DataKey::Config,
            &Config {
                admin,
                auction: None,
            },
        );
        Ok(())
    }

    pub fn set_auction(e: Env, auction: Address) -> Result<(), Error> {
        let mut config = Self::config(&e)?;
        config.admin.require_auth();
        config.auction = Some(auction);
        e.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    /// Put an exit at the back of the line. Returns its position (0 = next).
    pub fn enqueue(e: Env, asset: Address, exit_id: u64) -> Result<u32, Error> {
        Self::require_auction(&e)?;

        let mut queue = Self::read_queue(&e, &asset);
        if queue.len() >= MAX_QUEUE_DEPTH {
            return Err(Error::QueueFull);
        }
        if queue.iter().any(|id| id == exit_id) {
            return Err(Error::AlreadyQueued);
        }

        queue.push_back(exit_id);
        let position = queue.len() - 1;
        let depth = queue.len();
        Self::write_queue(&e, &asset, &queue);

        Queued {
            asset,
            exit_id,
            position,
            depth,
        }
        .publish(&e);
        Ok(position)
    }

    /// Take an exit out of the line. Everyone behind it moves up by one — which
    /// is the only way positions ever change.
    pub fn dequeue(e: Env, asset: Address, exit_id: u64) -> Result<(), Error> {
        Self::require_auction(&e)?;

        let queue = Self::read_queue(&e, &asset);
        let index = queue
            .iter()
            .position(|id| id == exit_id)
            .ok_or(Error::NotQueued)? as u32;

        let mut next = Vec::new(&e);
        for (i, id) in queue.iter().enumerate() {
            if i as u32 != index {
                next.push_back(id);
            }
        }
        let depth = next.len();
        Self::write_queue(&e, &asset, &next);

        Dequeued {
            asset,
            exit_id,
            depth,
        }
        .publish(&e);
        Ok(())
    }

    // ── queries ──

    pub fn position_of(e: Env, asset: Address, exit_id: u64) -> Option<u32> {
        Self::read_queue(&e, &asset)
            .iter()
            .position(|id| id == exit_id)
            .map(|p| p as u32)
    }

    pub fn head(e: Env, asset: Address) -> Option<u64> {
        Self::read_queue(&e, &asset).first()
    }

    pub fn depth(e: Env, asset: Address) -> u32 {
        Self::read_queue(&e, &asset).len()
    }

    pub fn list(e: Env, asset: Address) -> Vec<u64> {
        Self::read_queue(&e, &asset)
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

    fn require_auction(e: &Env) -> Result<(), Error> {
        let auction = Self::config(e)?.auction.ok_or(Error::NotAuction)?;
        auction.require_auth();
        Ok(())
    }

    fn read_queue(e: &Env, asset: &Address) -> Vec<u64> {
        let key = DataKey::Queue(asset.clone());
        match e.storage().persistent().get(&key) {
            Some(q) => {
                e.storage()
                    .persistent()
                    .extend_ttl(&key, LIFETIME_THRESHOLD, BUMP_AMOUNT);
                q
            }
            None => Vec::new(e),
        }
    }

    fn write_queue(e: &Env, asset: &Address, queue: &Vec<u64>) {
        let key = DataKey::Queue(asset.clone());
        e.storage().persistent().set(&key, queue);
        e.storage()
            .persistent()
            .extend_ttl(&key, LIFETIME_THRESHOLD, BUMP_AMOUNT);
    }
}

#[cfg(test)]
mod tests;
