#![cfg(test)]
extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

use crate::{Error, ExitAuction, ExitAuctionClient, Status, DEFAULT_DURATION};
use fifo_queue::{FifoQueue, FifoQueueClient};
use lp_vault::{Error as VaultError, LpVault, LpVaultClient, MIN_BACKING_AGE};
use settlement_router::{SettlementRouter, SettlementRouterClient};

const USDC: i128 = 10_000_000; // 1 USDC at 7 decimals
const SHARE: i128 = 10_000_000; // 1 participation token at 7 decimals
const FEE_BPS: u32 = 150;

/// 100 shares the seller says are worth 10,000 USDC.
const SIZE: i128 = 100 * SHARE;
const REFERENCE: i128 = 10_000 * USDC;

#[allow(dead_code)]
struct Fixture<'a> {
    e: Env,
    auction: ExitAuctionClient<'a>,
    vault: LpVaultClient<'a>,
    router: SettlementRouterClient<'a>,
    queue: FifoQueueClient<'a>,
    usdc: token::StellarAssetClient<'a>,
    usdc_bal: token::Client<'a>,
    asset: token::StellarAssetClient<'a>,
    asset_bal: token::Client<'a>,
    admin: Address,
    treasury: Address,
    seller: Address,
}

fn setup() -> Fixture<'static> {
    let e = Env::default();
    e.mock_all_auths();
    e.ledger().set_timestamp(1_000_000);

    let admin = Address::generate(&e);
    let issuer = Address::generate(&e);
    let treasury = Address::generate(&e);

    let usdc_sac = e.register_stellar_asset_contract_v2(issuer.clone());
    let asset_sac = e.register_stellar_asset_contract_v2(issuer.clone());

    let vault = LpVaultClient::new(&e, &e.register(LpVault, ()));
    let router = SettlementRouterClient::new(&e, &e.register(SettlementRouter, ()));
    let queue = FifoQueueClient::new(&e, &e.register(FifoQueue, ()));
    let auction = ExitAuctionClient::new(&e, &e.register(ExitAuction, ()));

    vault.initialize(
        &admin,
        &usdc_sac.address(),
        &(100 * USDC),
        &Some(10_000),
        &None,
    );
    router.initialize(
        &admin,
        &vault.address,
        &usdc_sac.address(),
        &treasury,
        &FEE_BPS,
    );
    queue.initialize(&admin);
    auction.initialize(&admin, &vault.address, &router.address, &queue.address);

    vault.set_auction(&auction.address);
    vault.set_router(&router.address);
    router.set_auction(&auction.address);
    queue.set_auction(&auction.address);

    let seller = Address::generate(&e);
    asset_sac_mint(&e, &asset_sac.address(), &seller, 10 * SIZE);

    Fixture {
        usdc: token::StellarAssetClient::new(&e, &usdc_sac.address()),
        usdc_bal: token::Client::new(&e, &usdc_sac.address()),
        asset: token::StellarAssetClient::new(&e, &asset_sac.address()),
        asset_bal: token::Client::new(&e, &asset_sac.address()),
        e,
        auction,
        vault,
        router,
        queue,
        admin,
        treasury,
        seller,
    }
}

fn asset_sac_mint(e: &Env, sac: &Address, to: &Address, amount: i128) {
    token::StellarAssetClient::new(e, sac).mint(to, &amount);
}

impl Fixture<'_> {
    /// A Liquidity Node with capital and an active backing on the asset. The
    /// backing still has to age before the vault will let it fill — `open`
    /// does that, so nodes are set up before the exit they answer.
    fn node(&self, deposit: i128, floor_bps: u32) -> Address {
        let node = Address::generate(&self.e);
        self.usdc.mint(&node, &deposit);
        self.vault.deposit(&node, &deposit);
        self.vault
            .set_appetite(&node, &self.asset.address, &deposit, &floor_bps, &true);
        node
    }

    /// Age every standing backing past the vault's minimum, then open. Keeps
    /// the bidding window clear of the ageing jump.
    fn open(&self, reserve: i128) -> u64 {
        self.advance(MIN_BACKING_AGE + 1);
        self.auction.open_exit(
            &self.seller,
            &self.asset.address,
            &SIZE,
            &REFERENCE,
            &reserve,
            &None,
        )
    }

    fn advance(&self, seconds: u64) {
        let now = self.e.ledger().timestamp();
        self.e.ledger().set_timestamp(now + seconds);
    }

    fn net_of_fee(&self, gross: i128) -> i128 {
        gross - gross * FEE_BPS as i128 / 10_000
    }
}

// ============================================================================
// Opening
// ============================================================================

#[test]
fn opening_an_exit_escrows_the_position() {
    let f = setup();
    let id = f.open(9_000 * USDC);

    assert_eq!(id, 1);
    assert_eq!(f.asset_bal.balance(&f.router.address), SIZE);
    assert_eq!(f.router.get_escrow(&id).unwrap().amount, SIZE);

    let exit = f.auction.get_exit(&id).unwrap();
    assert_eq!(exit.status, Status::Open);
    assert_eq!(exit.best_node, None);
    assert_eq!(exit.closes_at, exit.opened_at + DEFAULT_DURATION);
}

// ============================================================================
// Bidding
// ============================================================================

#[test]
fn the_highest_bid_wins_and_the_loser_gets_its_capital_back() {
    let f = setup();
    let low = f.node(10_000 * USDC, 0);
    let high = f.node(10_000 * USDC, 0);
    let id = f.open(0);

    f.auction.place_bid(&low, &id, &(9_000 * USDC));
    assert_eq!(f.vault.get_node(&low).committed, 9_000 * USDC);

    f.auction.place_bid(&high, &id, &(9_300 * USDC));

    // The outbid node's capital is free again the moment it loses.
    assert_eq!(f.vault.get_node(&low).committed, 0);
    assert_eq!(f.vault.free_balance(&low), 10_000 * USDC);
    assert_eq!(f.vault.get_node(&high).committed, 9_300 * USDC);
    assert_eq!(f.auction.get_exit(&id).unwrap().best_node, Some(high));
}

#[test]
fn matching_the_standing_bid_does_not_beat_it() {
    let f = setup();
    let first = f.node(10_000 * USDC, 0);
    let second = f.node(10_000 * USDC, 0);
    let id = f.open(0);

    f.auction.place_bid(&first, &id, &(9_000 * USDC));
    assert_eq!(
        f.auction.try_place_bid(&second, &id, &(9_000 * USDC)),
        Err(Ok(Error::BidTooLow))
    );
    assert_eq!(f.auction.get_exit(&id).unwrap().best_node, Some(first));
}

#[test]
fn a_bid_the_nodes_own_terms_forbid_never_becomes_the_best_bid() {
    let f = setup();
    // The node will not touch this asset under a 10% discount.
    let picky = f.node(10_000 * USDC, 1_000);
    let id = f.open(0);

    // 9,300 on a 10,000 reference is a 7% discount: below its own floor.
    assert_eq!(
        f.auction.try_place_bid(&picky, &id, &(9_300 * USDC)),
        Err(Err(soroban_sdk::InvokeError::Contract(
            VaultError::DiscountBelowFloor as u32
        )))
    );
    assert_eq!(f.auction.get_exit(&id).unwrap().best_node, None);

    // At 11% it fills, and the discount the event carries is the one quoted.
    let bid = 8_900 * USDC;
    assert_eq!(f.auction.quote_discount_bps(&id, &bid), 1_100);
    f.auction.place_bid(&picky, &id, &bid);
    assert_eq!(f.auction.get_exit(&id).unwrap().best_usdc, bid);
}

#[test]
fn bidding_stops_when_the_window_does() {
    let f = setup();
    let node = f.node(10_000 * USDC, 0);
    let id = f.open(0);
    f.advance(DEFAULT_DURATION + 1);

    assert_eq!(
        f.auction.try_place_bid(&node, &id, &(9_000 * USDC)),
        Err(Ok(Error::BiddingOver))
    );
}

// ============================================================================
// Settling
// ============================================================================

#[test]
fn accepting_a_bid_moves_both_legs_in_one_transaction() {
    let f = setup();
    let node = f.node(10_000 * USDC, 0);
    let id = f.open(9_000 * USDC);
    f.auction.place_bid(&node, &id, &(9_200 * USDC));

    f.auction.accept_bid(&id);

    assert_eq!(f.usdc_bal.balance(&f.seller), f.net_of_fee(9_200 * USDC));
    assert_eq!(
        f.usdc_bal.balance(&f.treasury),
        9_200 * USDC - f.net_of_fee(9_200 * USDC)
    );
    assert_eq!(f.asset_bal.balance(&node), SIZE);
    assert_eq!(f.asset_bal.balance(&f.router.address), 0);
    assert_eq!(f.auction.get_exit(&id).unwrap().status, Status::Settled);
    // The commitment was consumed, not released: the node holds the position.
    assert_eq!(f.vault.get_node(&node).committed, 0);
    assert_eq!(f.vault.get_node(&node).filled, 9_200 * USDC);
}

#[test]
fn a_settled_exit_is_finished_with() {
    let f = setup();
    let node = f.node(10_000 * USDC, 0);
    let id = f.open(0);
    f.auction.place_bid(&node, &id, &(9_200 * USDC));
    f.auction.accept_bid(&id);

    assert_eq!(f.auction.try_accept_bid(&id), Err(Ok(Error::ExitClosed)));
    assert_eq!(f.auction.try_cancel(&id), Err(Ok(Error::ExitClosed)));
    let other = f.node(10_000 * USDC, 0);
    assert_eq!(
        f.auction.try_place_bid(&other, &id, &(9_900 * USDC)),
        Err(Ok(Error::ExitClosed))
    );
}

#[test]
fn there_is_nothing_to_accept_without_a_bid() {
    let f = setup();
    let id = f.open(0);
    assert_eq!(f.auction.try_accept_bid(&id), Err(Ok(Error::NoBids)));
}

// ============================================================================
// The seller is never obligated
// ============================================================================

#[test]
fn cancelling_returns_the_position_and_frees_the_bidder() {
    let f = setup();
    let node = f.node(10_000 * USDC, 0);
    let id = f.open(9_000 * USDC);
    f.auction.place_bid(&node, &id, &(9_500 * USDC));

    f.auction.cancel(&id);

    assert_eq!(f.asset_bal.balance(&f.seller), 10 * SIZE);
    assert_eq!(f.asset_bal.balance(&f.router.address), 0);
    assert_eq!(f.vault.free_balance(&node), 10_000 * USDC);
    assert_eq!(f.usdc_bal.balance(&f.seller), 0);
    assert_eq!(f.auction.get_exit(&id).unwrap().status, Status::Cancelled);
}

// ============================================================================
// The queue
// ============================================================================

#[test]
fn a_window_that_closes_under_the_reserve_takes_a_place_in_line() {
    let f = setup();
    let node = f.node(10_000 * USDC, 0);
    let id = f.open(9_500 * USDC);
    f.auction.place_bid(&node, &id, &(9_000 * USDC));
    f.advance(DEFAULT_DURATION + 1);

    f.auction.close(&id);

    assert_eq!(f.auction.get_exit(&id).unwrap().status, Status::Queued);
    assert_eq!(f.queue.head(&f.asset.address), Some(id));
    // Waiting, not rejected: the tokens are still escrowed and the bid stands.
    assert_eq!(f.asset_bal.balance(&f.router.address), SIZE);
    assert_eq!(f.vault.get_node(&node).committed, 9_000 * USDC);
}

#[test]
fn a_queued_exit_still_takes_bids_and_still_fills() {
    let f = setup();
    let first = f.node(10_000 * USDC, 0);
    let id = f.open(9_500 * USDC);
    f.auction.place_bid(&first, &id, &(9_000 * USDC));
    f.advance(DEFAULT_DURATION + 1);
    f.auction.close(&id);

    // Capital shows up later and beats the reserve.
    let later = f.node(10_000 * USDC, 0);
    f.advance(MIN_BACKING_AGE + 1);
    f.auction.place_bid(&later, &id, &(9_600 * USDC));
    f.auction.accept_bid(&id);

    assert_eq!(f.usdc_bal.balance(&f.seller), f.net_of_fee(9_600 * USDC));
    assert_eq!(f.asset_bal.balance(&later), SIZE);
    assert_eq!(f.vault.free_balance(&first), 10_000 * USDC);
    // Settling took it out of the line.
    assert_eq!(f.queue.head(&f.asset.address), None);
}

#[test]
fn nobody_buys_their_way_past_the_head_of_the_line() {
    let f = setup();
    let waiting = f.open(9_500 * USDC);
    f.advance(DEFAULT_DURATION + 1);
    f.auction.close(&waiting); // no bids at all -> queued, at the head

    // A second exit on the same asset, with money behind it.
    let node = f.node(10_000 * USDC, 0);
    let jumper = f.open(0);
    f.auction.place_bid(&node, &jumper, &(9_800 * USDC));

    assert_eq!(f.auction.try_accept_bid(&jumper), Err(Ok(Error::NotAtHead)));

    // Closing it does not fill it either — it lines up behind.
    f.advance(DEFAULT_DURATION + 1);
    f.auction.close(&jumper);
    assert_eq!(f.auction.get_exit(&jumper).unwrap().status, Status::Queued);
    assert_eq!(f.queue.list(&f.asset.address).len(), 2);
    assert_eq!(f.queue.head(&f.asset.address), Some(waiting));

    // The seller ahead of it leaves; the line moves up; now it fills.
    f.auction.cancel(&waiting);
    assert_eq!(f.queue.head(&f.asset.address), Some(jumper));
    f.auction.accept_bid(&jumper);
    assert_eq!(f.auction.get_exit(&jumper).unwrap().status, Status::Settled);
    assert_eq!(f.asset_bal.balance(&node), SIZE);
}

#[test]
fn a_window_cannot_be_closed_early_and_cannot_be_closed_twice() {
    let f = setup();
    let id = f.open(0);
    assert_eq!(f.auction.try_close(&id), Err(Ok(Error::StillBidding)));

    f.advance(DEFAULT_DURATION + 1);
    f.auction.close(&id);
    assert_eq!(f.auction.try_close(&id), Err(Ok(Error::ExitClosed)));
}

#[test]
fn a_window_that_clears_the_reserve_settles_itself_when_it_closes() {
    let f = setup();
    let node = f.node(10_000 * USDC, 0);
    let id = f.open(9_000 * USDC);
    f.auction.place_bid(&node, &id, &(9_400 * USDC));
    f.advance(DEFAULT_DURATION + 1);

    f.auction.close(&id);

    assert_eq!(f.auction.get_exit(&id).unwrap().status, Status::Settled);
    assert_eq!(f.usdc_bal.balance(&f.seller), f.net_of_fee(9_400 * USDC));
    assert_eq!(f.queue.head(&f.asset.address), None);
}

// ============================================================================
// Queues are per asset
// ============================================================================

#[test]
fn a_line_on_one_asset_does_not_hold_up_another() {
    let f = setup();
    let waiting = f.open(9_500 * USDC);
    f.advance(DEFAULT_DURATION + 1);
    f.auction.close(&waiting);

    // A different asset entirely, with its own book.
    let issuer = Address::generate(&f.e);
    let other_sac = f.e.register_stellar_asset_contract_v2(issuer);
    asset_sac_mint(&f.e, &other_sac.address(), &f.seller, SIZE);

    let node = Address::generate(&f.e);
    f.usdc.mint(&node, &(10_000 * USDC));
    f.vault.deposit(&node, &(10_000 * USDC));
    f.vault
        .set_appetite(&node, &other_sac.address(), &(10_000 * USDC), &0, &true);
    f.advance(MIN_BACKING_AGE + 1);

    let other = f.auction.open_exit(
        &f.seller,
        &other_sac.address(),
        &SIZE,
        &REFERENCE,
        &0,
        &None,
    );
    f.auction.place_bid(&node, &other, &(9_700 * USDC));
    f.auction.accept_bid(&other);

    assert_eq!(f.auction.get_exit(&other).unwrap().status, Status::Settled);
    assert_eq!(f.auction.get_exit(&waiting).unwrap().status, Status::Queued);
}
