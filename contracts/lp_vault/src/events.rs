use soroban_sdk::{contractevent, Address};

/// `vault.deposit` — a Liquidity Node added USDC.
#[contractevent]
pub struct Deposit {
    #[topic]
    pub node: Address,
    pub amount: i128,
    pub total_deposited: i128,
}

/// `vault.withdraw` — capital left the vault, after the timelock and the free-balance check.
#[contractevent]
pub struct Withdraw {
    #[topic]
    pub node: Address,
    pub amount: i128,
    pub total_deposited: i128,
}

/// A withdrawal was requested and is now serving its timelock.
#[contractevent]
pub struct WithdrawRequested {
    #[topic]
    pub node: Address,
    pub amount: i128,
    pub unlock_at: u64,
}

#[contractevent]
pub struct WithdrawCancelled {
    #[topic]
    pub node: Address,
    pub amount: i128,
}

/// A node declared (or updated) what it will do for an asset.
#[contractevent]
pub struct AppetiteSet {
    #[topic]
    pub node: Address,
    #[topic]
    pub asset: Address,
    pub max_exposure: i128,
    pub min_discount_bps: u32,
    pub active: bool,
}

/// Capital locked behind a live bid.
#[contractevent]
pub struct Committed {
    #[topic]
    pub node: Address,
    #[topic]
    pub asset: Address,
    pub amount: i128,
}

/// Capital freed because the bid was outbid, cancelled or expired.
#[contractevent]
pub struct Released {
    #[topic]
    pub node: Address,
    #[topic]
    pub asset: Address,
    pub amount: i128,
}

/// Capital actually left the vault to pay a seller.
#[contractevent]
pub struct PaidOut {
    #[topic]
    pub node: Address,
    #[topic]
    pub asset: Address,
    pub to: Address,
    pub amount: i128,
}
