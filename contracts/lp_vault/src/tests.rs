#![cfg(test)]
extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

use crate::{contract::LpVault, contract::LpVaultClient, errors::Error, storage::MIN_BACKING_AGE};

const USDC: i128 = 10_000_000; // 1 USDC at 7 decimals

#[allow(dead_code)]
struct Fixture<'a> {
    e: Env,
    vault: LpVaultClient<'a>,
    usdc: token::StellarAssetClient<'a>,
    admin: Address,
    auction: Address,
    router: Address,
    asset: Address,
}

fn setup() -> Fixture<'static> {
    let e = Env::default();
    e.mock_all_auths();
    e.ledger().set_timestamp(1_000_000);

    let admin = Address::generate(&e);
    let issuer = Address::generate(&e);
    let sac = e.register_stellar_asset_contract_v2(issuer.clone());
    let usdc = token::StellarAssetClient::new(&e, &sac.address());

    let vault_id = e.register(LpVault, ());
    let vault = LpVaultClient::new(&e, &vault_id);
    vault.initialize(&admin, &sac.address(), &(100 * USDC), &None, &None);

    let auction = Address::generate(&e);
    let router = Address::generate(&e);
    vault.set_auction(&auction);
    vault.set_router(&router);
    let asset = Address::generate(&e);

    Fixture {
        e,
        vault,
        usdc,
        admin,
        auction,
        router,
        asset,
    }
}

impl Fixture<'_> {
    /// A funded node with an aged, active backing on `asset`.
    fn node_with_backing(&self, deposit: i128, max_exposure: i128, floor_bps: u32) -> Address {
        let node = Address::generate(&self.e);
        self.usdc.mint(&node, &deposit);
        self.vault.deposit(&node, &deposit);
        self.vault
            .set_appetite(&node, &self.asset, &max_exposure, &floor_bps, &true);
        self.age_backing();
        node
    }

    fn age_backing(&self) {
        let now = self.e.ledger().timestamp();
        self.e.ledger().set_timestamp(now + MIN_BACKING_AGE + 1);
    }
}

// ============================================================================
// Lifecycle
// ============================================================================

#[test]
fn initialize_is_once_only() {
    let f = setup();
    let res = f
        .vault
        .try_initialize(&f.admin, &f.usdc.address, &0, &None, &None);
    assert_eq!(res, Err(Ok(Error::AlreadyInitialized)));
}

#[test]
fn deposit_below_min_is_rejected_only_for_the_first_one() {
    let f = setup();
    let node = Address::generate(&f.e);
    f.usdc.mint(&node, &(500 * USDC));

    assert_eq!(
        f.vault.try_deposit(&node, &(50 * USDC)),
        Err(Ok(Error::BelowMinDeposit))
    );

    f.vault.deposit(&node, &(100 * USDC));
    // Topping up an existing node is not subject to the minimum.
    f.vault.deposit(&node, &(1 * USDC));
    assert_eq!(f.vault.get_node(&node).deposited, 101 * USDC);
}

// ============================================================================
// The hard invariant: free balance
// ============================================================================

#[test]
fn commit_cannot_exceed_free_balance() {
    let f = setup();
    // Single-fill cap defaults to 30%, so raise it out of the way for this test.
    f.vault.set_max_single_fill_bps(&10_000);
    let node = f.node_with_backing(1_000 * USDC, 10_000 * USDC, 100);

    f.vault
        .commit(&node, &f.asset, &(600 * USDC), &500);
    assert_eq!(f.vault.free_balance(&node), 400 * USDC);

    assert_eq!(
        f.vault
            .try_commit(&node, &f.asset, &(401 * USDC), &500),
        Err(Ok(Error::InsufficientFreeBalance))
    );
}

#[test]
fn a_pending_withdrawal_is_reserved_against_new_commitments() {
    let f = setup();
    f.vault.set_max_single_fill_bps(&10_000);
    let node = f.node_with_backing(1_000 * USDC, 10_000 * USDC, 100);

    f.vault.request_withdrawal(&node, &(700 * USDC));
    assert_eq!(f.vault.free_balance(&node), 300 * USDC);

    assert_eq!(
        f.vault
            .try_commit(&node, &f.asset, &(400 * USDC), &500),
        Err(Ok(Error::InsufficientFreeBalance))
    );
    f.vault
        .commit(&node, &f.asset, &(300 * USDC), &500);
}

#[test]
fn a_requested_withdrawal_survives_everything_committed_after_it() {
    let f = setup();
    f.vault.set_max_single_fill_bps(&10_000);
    let node = f.node_with_backing(1_000 * USDC, 10_000 * USDC, 100);
    let seller = Address::generate(&f.e);

    f.vault.request_withdrawal(&node, &(500 * USDC));
    assert_eq!(
        f.vault.try_execute_withdrawal(&node),
        Err(Ok(Error::TimelockNotElapsed))
    );

    // The reservation is what makes this safe: the node can commit its remaining
    // 500 and see it paid out, and the withdrawal is still fully covered after.
    f.vault.commit(&node, &f.asset, &(500 * USDC), &500);
    f.vault
        .pay_out(&node, &f.asset, &seller, &(500 * USDC));

    let now = f.e.ledger().timestamp();
    f.e.ledger().set_timestamp(now + 24 * 60 * 60 + 1);
    f.vault.execute_withdrawal(&node);

    let state = f.vault.get_node(&node);
    assert_eq!(state.deposited, 0);
    assert_eq!(state.filled, 500 * USDC);

    let sac = token::Client::new(&f.e, &f.usdc.address);
    assert_eq!(sac.balance(&node), 500 * USDC);
    assert_eq!(sac.balance(&seller), 500 * USDC);
    // The vault is empty and never owed more than it held.
    assert_eq!(sac.balance(&f.vault.address), 0);
}

#[test]
fn cancelling_a_withdrawal_gives_the_capital_back_to_the_book() {
    let f = setup();
    f.vault.set_max_single_fill_bps(&10_000);
    let node = f.node_with_backing(1_000 * USDC, 10_000 * USDC, 100);

    f.vault.request_withdrawal(&node, &(900 * USDC));
    assert_eq!(f.vault.quote_capacity(&node, &f.asset, &500), 100 * USDC);

    f.vault.cancel_withdrawal(&node);
    assert_eq!(f.vault.quote_capacity(&node, &f.asset, &500), 1_000 * USDC);
    assert_eq!(
        f.vault.try_cancel_withdrawal(&node),
        Err(Ok(Error::NoWithdrawalPending))
    );
}

// ============================================================================
// Standing terms
// ============================================================================

#[test]
fn a_bid_below_the_nodes_own_floor_is_refused() {
    let f = setup();
    let node = f.node_with_backing(1_000 * USDC, 10_000 * USDC, 450);

    assert_eq!(
        f.vault
            .try_commit(&node, &f.asset, &(100 * USDC), &449),
        Err(Ok(Error::DiscountBelowFloor))
    );
    f.vault
        .commit(&node, &f.asset, &(100 * USDC), &450);
}

#[test]
fn a_fresh_backing_cannot_fill_yet() {
    let f = setup();
    let node = Address::generate(&f.e);
    f.usdc.mint(&node, &(1_000 * USDC));
    f.vault.deposit(&node, &(1_000 * USDC));
    f.vault
        .set_appetite(&node, &f.asset, &(10_000 * USDC), &100, &true);

    assert_eq!(
        f.vault
            .try_commit(&node, &f.asset, &(100 * USDC), &500),
        Err(Ok(Error::BackingTooYoung))
    );
    assert_eq!(f.vault.quote_capacity(&node, &f.asset, &500), 0);

    f.age_backing();
    f.vault
        .commit(&node, &f.asset, &(100 * USDC), &500);
}

#[test]
fn exposure_ceiling_is_enforced() {
    let f = setup();
    f.vault.set_max_single_fill_bps(&10_000);
    let node = f.node_with_backing(1_000 * USDC, 300 * USDC, 100);

    f.vault
        .commit(&node, &f.asset, &(300 * USDC), &500);
    assert_eq!(
        f.vault.try_commit(&node, &f.asset, &(1 * USDC), &500),
        Err(Ok(Error::ExposureExceeded))
    );
}

#[test]
fn one_exit_cannot_take_more_than_the_single_fill_cap() {
    let f = setup();
    // Default cap is 30% of deposits.
    let node = f.node_with_backing(1_000 * USDC, 10_000 * USDC, 100);

    assert_eq!(
        f.vault
            .try_commit(&node, &f.asset, &(301 * USDC), &500),
        Err(Ok(Error::SingleFillCapExceeded))
    );
    f.vault
        .commit(&node, &f.asset, &(300 * USDC), &500);
    assert_eq!(f.vault.quote_capacity(&node, &f.asset, &500), 300 * USDC);
}

#[test]
fn deactivating_appetite_stops_new_fills_without_touching_capital() {
    let f = setup();
    let node = f.node_with_backing(1_000 * USDC, 10_000 * USDC, 100);
    f.vault
        .set_appetite(&node, &f.asset, &(10_000 * USDC), &100, &false);

    assert_eq!(
        f.vault
            .try_commit(&node, &f.asset, &(100 * USDC), &500),
        Err(Ok(Error::AppetiteInactive))
    );
    assert_eq!(f.vault.free_balance(&node), 1_000 * USDC);
}

#[test]
fn re_arming_a_backing_restarts_the_age_clock() {
    let f = setup();
    let node = f.node_with_backing(1_000 * USDC, 10_000 * USDC, 100);
    // Off then on again: the clock restarts, so a node cannot park a dormant
    // backing and flip it live the instant it sees an exit it likes.
    f.vault
        .set_appetite(&node, &f.asset, &(10_000 * USDC), &100, &false);
    f.vault
        .set_appetite(&node, &f.asset, &(10_000 * USDC), &100, &true);

    assert_eq!(
        f.vault
            .try_commit(&node, &f.asset, &(100 * USDC), &500),
        Err(Ok(Error::BackingTooYoung))
    );
}

// ============================================================================
// Settlement
// ============================================================================

#[test]
fn pay_out_moves_usdc_and_keeps_the_position_on_the_books() {
    let f = setup();
    let node = f.node_with_backing(1_000 * USDC, 10_000 * USDC, 100);
    let seller = Address::generate(&f.e);

    f.vault
        .commit(&node, &f.asset, &(300 * USDC), &500);
    f.vault
        .pay_out(&node, &f.asset, &seller, &(300 * USDC));

    let sac = token::Client::new(&f.e, &f.usdc.address);
    assert_eq!(sac.balance(&seller), 300 * USDC);

    let state = f.vault.get_node(&node);
    assert_eq!(state.deposited, 700 * USDC);
    assert_eq!(state.committed, 0);
    assert_eq!(state.filled, 300 * USDC);

    // The node now holds the asset, so the exposure stays until it says otherwise.
    assert_eq!(f.vault.get_appetite(&node, &f.asset).exposure, 300 * USDC);
    f.vault.reduce_exposure(&node, &f.asset, &(300 * USDC));
    assert_eq!(f.vault.get_appetite(&node, &f.asset).exposure, 0);
}

#[test]
fn release_returns_the_capital_and_the_headroom() {
    let f = setup();
    let node = f.node_with_backing(1_000 * USDC, 10_000 * USDC, 100);

    f.vault
        .commit(&node, &f.asset, &(300 * USDC), &500);
    f.vault.release(&node, &f.asset, &(300 * USDC));

    assert_eq!(f.vault.get_node(&node).committed, 0);
    assert_eq!(f.vault.get_appetite(&node, &f.asset).exposure, 0);
    assert_eq!(f.vault.free_balance(&node), 1_000 * USDC);
}

#[test]
fn nothing_can_be_released_or_paid_beyond_what_was_committed() {
    let f = setup();
    let node = f.node_with_backing(1_000 * USDC, 10_000 * USDC, 100);
    let seller = Address::generate(&f.e);

    f.vault
        .commit(&node, &f.asset, &(100 * USDC), &500);
    assert_eq!(
        f.vault.try_release(&node, &f.asset, &(101 * USDC)),
        Err(Ok(Error::CommitUnderflow))
    );
    assert_eq!(
        f.vault
            .try_pay_out(&node, &f.asset, &seller, &(101 * USDC)),
        Err(Ok(Error::CommitUnderflow))
    );
}

// ============================================================================
// Wiring
// ============================================================================

#[test]
fn commit_and_pay_out_need_the_wired_contracts() {
    let e = Env::default();
    e.mock_all_auths();
    let admin = Address::generate(&e);
    let issuer = Address::generate(&e);
    let sac = e.register_stellar_asset_contract_v2(issuer);

    let vault = LpVaultClient::new(&e, &e.register(LpVault, ()));
    vault.initialize(&admin, &sac.address(), &0, &None, &None);

    let node = Address::generate(&e);
    let asset = Address::generate(&e);
    assert_eq!(
        vault.try_commit(&node, &asset, &(1 * USDC), &500),
        Err(Ok(Error::NotWired))
    );
    assert_eq!(
        vault.try_pay_out(&node, &asset, &node, &(1 * USDC)),
        Err(Ok(Error::NotWired))
    );
}

#[test]
fn quote_capacity_is_the_whole_book_in_one_call() {
    let f = setup();
    let node = f.node_with_backing(1_000 * USDC, 200 * USDC, 300);

    // Below the node's floor: it does not participate at all.
    assert_eq!(f.vault.quote_capacity(&node, &f.asset, &299), 0);
    // At or above the floor: capped by exposure headroom (200) rather than by
    // free balance (1000) or the single-fill cap (300).
    assert_eq!(f.vault.quote_capacity(&node, &f.asset, &300), 200 * USDC);

    let other = Address::generate(&f.e);
    assert_eq!(f.vault.quote_capacity(&other, &f.asset, &300), 0);
}
