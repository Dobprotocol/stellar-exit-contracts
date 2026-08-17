use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    // ── lifecycle ──
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NotAdmin = 3,
    /// The caller is not the exit_auction contract wired into this vault.
    NotAuction = 4,
    /// The caller is not the settlement_router contract wired into this vault.
    NotRouter = 5,
    /// A collaborating contract address was never configured.
    NotWired = 6,

    // ── deposits / withdrawals ──
    InvalidAmount = 10,
    BelowMinDeposit = 11,
    UnknownNode = 12,
    /// Free balance (deposited − committed) does not cover the request.
    InsufficientFreeBalance = 13,
    WithdrawalPending = 14,
    NoWithdrawalPending = 15,
    /// The timelock on a withdrawal request has not elapsed yet.
    TimelockNotElapsed = 16,

    // ── appetite ──
    NoAppetite = 20,
    AppetiteInactive = 21,
    /// This fill would push the node past the exposure ceiling it set for the asset.
    ExposureExceeded = 22,
    /// The bid discount is below the floor this node declared for the asset.
    DiscountBelowFloor = 23,
    /// The backing has not aged past MIN_BACKING_AGE yet.
    BackingTooYoung = 24,

    // ── fill guards ──
    /// One exit may not consume more than `max_single_fill_bps` of a node's capital.
    SingleFillCapExceeded = 30,
    /// Releasing/paying more than what is actually committed.
    CommitUnderflow = 31,

    // ── config ──
    InvalidBps = 40,
}
