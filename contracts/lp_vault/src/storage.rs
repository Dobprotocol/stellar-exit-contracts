use soroban_sdk::{contracttype, Address, Env, IntoVal, Val};

use crate::errors::Error;

const DAY_IN_LEDGERS: u32 = 17_280;

const INSTANCE_BUMP_AMOUNT: u32 = 7 * DAY_IN_LEDGERS;
const INSTANCE_LIFETIME_THRESHOLD: u32 = INSTANCE_BUMP_AMOUNT - DAY_IN_LEDGERS;

const PERSISTENT_BUMP_AMOUNT: u32 = 30 * DAY_IN_LEDGERS;
const PERSISTENT_LIFETIME_THRESHOLD: u32 = PERSISTENT_BUMP_AMOUNT - DAY_IN_LEDGERS;

pub const BPS_DENOMINATOR: i128 = 10_000;

/// A backing must age before it can fill, so a node cannot front-run a single
/// known exit by registering appetite in the same ledger and withdrawing after.
pub const MIN_BACKING_AGE: u64 = 3_600;

/// Default ceiling on how much of one node's capital a single exit may consume.
/// This is the "hot reserve floor" from the spec, expressed as a per-fill cap:
/// it stops one large exit from emptying a node rather than sterilising capital.
pub const DEFAULT_MAX_SINGLE_FILL_BPS: u32 = 3_000; // 30%

/// Default cooldown between asking to withdraw and being able to.
pub const DEFAULT_WITHDRAW_TIMELOCK: u64 = 24 * 60 * 60;

// ============================================================================
// Types
// ============================================================================

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Config {
    pub admin: Address,
    /// USDC token contract (the settlement asset).
    pub usdc: Address,
    /// exit_auction — the only contract allowed to commit/release capital.
    pub auction: Option<Address>,
    /// settlement_router — the only contract allowed to move capital out.
    pub router: Option<Address>,
    pub min_deposit: i128,
    pub max_single_fill_bps: u32,
    pub withdraw_timelock: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Node {
    /// USDC the node has in the vault and has not withdrawn.
    pub deposited: i128,
    /// Subset of `deposited` locked behind live bids. Never exceeds `deposited`.
    pub committed: i128,
    /// Cumulative USDC actually paid out to sellers. Bookkeeping only.
    pub filled: i128,
    pub joined_at: u64,
}

/// What a node is willing to do for one asset. This is the standing order:
/// the node names its floor discount and its ceiling exposure, and the auction
/// enforces both. Everything about *why* those numbers — TRUFA score, the
/// operator's own risk model — is decided off-chain by the node.
#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct Appetite {
    /// Maximum USDC this node will have outstanding against this asset.
    pub max_exposure: i128,
    /// USDC currently outstanding (committed to live bids + already paid out).
    pub exposure: i128,
    /// Floor discount, in bps off the reference price. The node will not fill tighter.
    pub min_discount_bps: u32,
    pub active: bool,
    pub backed_at: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct WithdrawRequest {
    pub amount: i128,
    pub unlock_at: u64,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Config,
    Node(Address),
    Appetite(Address, Address), // (node, asset)
    Withdrawal(Address),
}

// ============================================================================
// TTL helpers
// ============================================================================

pub fn bump_instance(e: &Env) {
    e.storage()
        .instance()
        .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
}

fn bump_persistent<K>(e: &Env, key: &K)
where
    K: IntoVal<Env, Val>,
{
    e.storage()
        .persistent()
        .extend_ttl(key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
}

// ============================================================================
// Accessors
// ============================================================================

pub fn has_config(e: &Env) -> bool {
    e.storage().instance().has(&DataKey::Config)
}

pub fn read_config(e: &Env) -> Result<Config, Error> {
    e.storage()
        .instance()
        .get(&DataKey::Config)
        .ok_or(Error::NotInitialized)
}

pub fn write_config(e: &Env, config: &Config) {
    e.storage().instance().set(&DataKey::Config, config);
    bump_instance(e);
}

pub fn read_node(e: &Env, node: &Address) -> Option<Node> {
    let key = DataKey::Node(node.clone());
    let found: Option<Node> = e.storage().persistent().get(&key);
    if found.is_some() {
        bump_persistent(e, &key);
    }
    found
}

pub fn write_node(e: &Env, node: &Address, state: &Node) {
    let key = DataKey::Node(node.clone());
    e.storage().persistent().set(&key, state);
    bump_persistent(e, &key);
}

pub fn read_appetite(e: &Env, node: &Address, asset: &Address) -> Option<Appetite> {
    let key = DataKey::Appetite(node.clone(), asset.clone());
    let found: Option<Appetite> = e.storage().persistent().get(&key);
    if found.is_some() {
        bump_persistent(e, &key);
    }
    found
}

pub fn write_appetite(e: &Env, node: &Address, asset: &Address, appetite: &Appetite) {
    let key = DataKey::Appetite(node.clone(), asset.clone());
    e.storage().persistent().set(&key, appetite);
    bump_persistent(e, &key);
}

pub fn read_withdrawal(e: &Env, node: &Address) -> Option<WithdrawRequest> {
    e.storage()
        .persistent()
        .get(&DataKey::Withdrawal(node.clone()))
}

pub fn write_withdrawal(e: &Env, node: &Address, req: &WithdrawRequest) {
    let key = DataKey::Withdrawal(node.clone());
    e.storage().persistent().set(&key, req);
    bump_persistent(e, &key);
}

pub fn clear_withdrawal(e: &Env, node: &Address) {
    e.storage()
        .persistent()
        .remove(&DataKey::Withdrawal(node.clone()));
}
