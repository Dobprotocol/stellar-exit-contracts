extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

use crate::{Error, SettlementRouter, SettlementRouterClient, MAX_PROTOCOL_FEE_BPS};
use lp_vault::{LpVault, LpVaultClient, MIN_BACKING_AGE};

const USDC: i128 = 10_000_000; // 1 USDC at 7 decimals
const SHARE: i128 = 10_000_000; // 1 participation token at 7 decimals
const FEE_BPS: u32 = 150; // 1.5%, the registry fee the app already quotes

#[allow(dead_code)]
struct Fixture<'a> {
    e: Env,
    router: SettlementRouterClient<'a>,
    vault: LpVaultClient<'a>,
    usdc: token::StellarAssetClient<'a>,
    usdc_bal: token::Client<'a>,
    asset: token::StellarAssetClient<'a>,
    asset_bal: token::Client<'a>,
    admin: Address,
    treasury: Address,
    auction: Address,
    seller: Address,
}

fn setup() -> Fixture<'static> {
    let e = Env::default();
    e.mock_all_auths();
    e.ledger().set_timestamp(1_000_000);

    let admin = Address::generate(&e);
    let issuer = Address::generate(&e);
    let treasury = Address::generate(&e);
    let auction = Address::generate(&e);

    let usdc_sac = e.register_stellar_asset_contract_v2(issuer.clone());
    let asset_sac = e.register_stellar_asset_contract_v2(issuer.clone());

    let vault = LpVaultClient::new(&e, &e.register(LpVault, ()));
    // The single-fill cap is the vault's own concern and has its own tests; here
    // it is opened up so settlement is the only thing under test.
    vault.initialize(
        &admin,
        &usdc_sac.address(),
        &(100 * USDC),
        &Some(10_000),
        &None,
    );

    let router = SettlementRouterClient::new(&e, &e.register(SettlementRouter, ()));
    router.initialize(
        &admin,
        &vault.address,
        &usdc_sac.address(),
        &treasury,
        &FEE_BPS,
    );

    vault.set_auction(&auction);
    vault.set_router(&router.address);
    router.set_auction(&auction);

    let seller = Address::generate(&e);

    Fixture {
        usdc: token::StellarAssetClient::new(&e, &usdc_sac.address()),
        usdc_bal: token::Client::new(&e, &usdc_sac.address()),
        asset: token::StellarAssetClient::new(&e, &asset_sac.address()),
        asset_bal: token::Client::new(&e, &asset_sac.address()),
        e,
        router,
        vault,
        admin,
        treasury,
        auction,
        seller,
    }
}

impl Fixture<'_> {
    /// A node with capital, an aged backing on the asset, and `committed`
    /// already reserved by the auction — the state a winning bid leaves behind.
    fn winning_node(&self, deposit: i128, committed: i128, discount_bps: u32) -> Address {
        let node = Address::generate(&self.e);
        self.usdc.mint(&node, &deposit);
        self.vault.deposit(&node, &deposit);
        self.vault
            .set_appetite(&node, &self.asset.address, &deposit, &0, &true);
        let now = self.e.ledger().timestamp();
        self.e.ledger().set_timestamp(now + MIN_BACKING_AGE + 1);
        self.vault
            .commit(&node, &self.asset.address, &committed, &discount_bps);
        node
    }

    /// A seller holding `amount` participation tokens with an open exit.
    fn open_exit(&self, exit_id: u64, amount: i128) {
        self.asset.mint(&self.seller, &amount);
        self.router
            .escrow(&exit_id, &self.seller, &self.asset.address, &amount);
    }
}

// ============================================================================
// Escrow
// ============================================================================

#[test]
fn escrow_takes_the_tokens_off_the_seller() {
    let f = setup();
    f.open_exit(1, 100 * SHARE);

    assert_eq!(f.asset_bal.balance(&f.seller), 0);
    assert_eq!(f.asset_bal.balance(&f.router.address), 100 * SHARE);
    assert_eq!(f.router.get_escrow(&1).unwrap().amount, 100 * SHARE);
}

#[test]
fn the_same_exit_cannot_escrow_twice() {
    let f = setup();
    f.open_exit(1, 100 * SHARE);
    f.asset.mint(&f.seller, &(100 * SHARE));

    assert_eq!(
        f.router
            .try_escrow(&1, &f.seller, &f.asset.address, &(100 * SHARE)),
        Err(Ok(Error::AlreadyEscrowed))
    );
}

#[test]
fn only_the_auction_moves_value() {
    let e = Env::default();
    e.mock_all_auths();
    let admin = Address::generate(&e);
    let router = SettlementRouterClient::new(&e, &e.register(SettlementRouter, ()));
    router.initialize(
        &admin,
        &Address::generate(&e),
        &Address::generate(&e),
        &Address::generate(&e),
        &FEE_BPS,
    );

    // No auction wired: every value-moving entry point is closed, admin included.
    let who = Address::generate(&e);
    assert_eq!(
        router.try_escrow(&1, &who, &Address::generate(&e), &1),
        Err(Ok(Error::NotWired))
    );
    assert_eq!(
        router.try_settle(&1, &who, &1, &1),
        Err(Ok(Error::NotWired))
    );
    assert_eq!(router.try_refund(&1), Err(Ok(Error::NotWired)));
}

// ============================================================================
// Settlement
// ============================================================================

#[test]
fn settlement_moves_both_legs_and_takes_the_fee_out_of_the_sellers_side() {
    let f = setup();
    f.open_exit(1, 100 * SHARE);
    // Node bids 9,200 USDC for 100 shares worth 10,000 at reference: an 8% discount.
    let node = f.winning_node(10_000 * USDC, 9_200 * USDC, 800);

    f.router.settle(&1, &node, &(100 * SHARE), &(9_200 * USDC));

    let fee = 9_200 * USDC * FEE_BPS as i128 / 10_000;
    assert_eq!(f.usdc_bal.balance(&f.seller), 9_200 * USDC - fee);
    assert_eq!(f.usdc_bal.balance(&f.treasury), fee);
    // The node paid the full gross: the fee comes out of the bid, not on top.
    assert_eq!(
        f.usdc_bal.balance(&f.vault.address),
        10_000 * USDC - 9_200 * USDC
    );

    // Asset leg: the node holds the position now, the router holds nothing.
    assert_eq!(f.asset_bal.balance(&node), 100 * SHARE);
    assert_eq!(f.asset_bal.balance(&f.router.address), 0);
    assert_eq!(f.router.get_escrow(&1), None);

    // And the seller's quoted number is the number that landed.
    assert_eq!(f.router.quote_net(&(9_200 * USDC)), 9_200 * USDC - fee);
}

#[test]
fn a_settled_exit_cannot_be_settled_again() {
    let f = setup();
    f.open_exit(1, 100 * SHARE);
    let node = f.winning_node(10_000 * USDC, 9_200 * USDC, 800);
    f.router.settle(&1, &node, &(100 * SHARE), &(9_200 * USDC));

    let second = f.winning_node(10_000 * USDC, 9_200 * USDC, 800);
    assert_eq!(
        f.router
            .try_settle(&1, &second, &(100 * SHARE), &(9_200 * USDC)),
        Err(Ok(Error::NothingEscrowed))
    );
}

#[test]
fn nothing_can_be_paid_for_tokens_that_were_never_escrowed() {
    let f = setup();
    f.open_exit(1, 100 * SHARE);
    let node = f.winning_node(10_000 * USDC, 9_200 * USDC, 800);

    assert_eq!(
        f.router
            .try_settle(&1, &node, &(101 * SHARE), &(9_200 * USDC)),
        Err(Ok(Error::ExceedsEscrow))
    );
    // A different exit id has no escrow at all.
    assert_eq!(
        f.router.try_settle(&2, &node, &SHARE, &USDC),
        Err(Ok(Error::NothingEscrowed))
    );
}

#[test]
fn the_book_can_fill_one_exit_across_several_nodes() {
    let f = setup();
    f.open_exit(1, 100 * SHARE);

    // Best price first: 60 shares at a 5% discount, the rest at 9%.
    let tight = f.winning_node(10_000 * USDC, 5_700 * USDC, 500);
    let wide = f.winning_node(10_000 * USDC, 3_640 * USDC, 900);

    f.router.settle(&1, &tight, &(60 * SHARE), &(5_700 * USDC));
    assert_eq!(f.router.get_escrow(&1).unwrap().amount, 40 * SHARE);

    f.router.settle(&1, &wide, &(40 * SHARE), &(3_640 * USDC));
    assert_eq!(f.router.get_escrow(&1), None);

    let gross = 5_700 * USDC + 3_640 * USDC;
    let fee = 5_700 * USDC * FEE_BPS as i128 / 10_000 + 3_640 * USDC * FEE_BPS as i128 / 10_000;
    assert_eq!(f.usdc_bal.balance(&f.seller), gross - fee);
    assert_eq!(f.asset_bal.balance(&tight), 60 * SHARE);
    assert_eq!(f.asset_bal.balance(&wide), 40 * SHARE);
}

#[test]
fn a_partial_fill_leaves_the_rest_refundable() {
    let f = setup();
    f.open_exit(1, 100 * SHARE);
    let node = f.winning_node(10_000 * USDC, 5_700 * USDC, 500);

    f.router.settle(&1, &node, &(60 * SHARE), &(5_700 * USDC));
    f.router.refund(&1);

    assert_eq!(f.asset_bal.balance(&f.seller), 40 * SHARE);
    assert_eq!(f.asset_bal.balance(&f.router.address), 0);
    assert_eq!(f.router.get_escrow(&1), None);
}

#[test]
fn a_node_that_never_committed_cannot_be_settled_against() {
    let f = setup();
    f.open_exit(1, 100 * SHARE);
    // Capital and appetite, but the auction never reserved anything.
    let node = Address::generate(&f.e);
    f.usdc.mint(&node, &(10_000 * USDC));
    f.vault.deposit(&node, &(10_000 * USDC));
    f.vault
        .set_appetite(&node, &f.asset.address, &(10_000 * USDC), &0, &true);

    assert!(f
        .router
        .try_settle(&1, &node, &(100 * SHARE), &(9_200 * USDC))
        .is_err());
    // The failed cash leg took the asset leg down with it.
    assert_eq!(f.asset_bal.balance(&f.router.address), 100 * SHARE);
    assert_eq!(f.usdc_bal.balance(&f.seller), 0);
}

// ============================================================================
// Refund
// ============================================================================

#[test]
fn a_cancelled_exit_returns_everything_to_the_seller() {
    let f = setup();
    f.open_exit(1, 100 * SHARE);

    f.router.refund(&1);

    assert_eq!(f.asset_bal.balance(&f.seller), 100 * SHARE);
    assert_eq!(f.router.get_escrow(&1), None);
    assert_eq!(f.router.try_refund(&1), Err(Ok(Error::NothingEscrowed)));
}

// ============================================================================
// Fee policy
// ============================================================================

#[test]
fn the_protocol_fee_has_a_ceiling_the_admin_cannot_pass() {
    let f = setup();
    assert_eq!(
        f.router
            .try_set_protocol_fee_bps(&(MAX_PROTOCOL_FEE_BPS + 1)),
        Err(Ok(Error::InvalidBps))
    );
    f.router.set_protocol_fee_bps(&MAX_PROTOCOL_FEE_BPS);
    assert_eq!(f.router.get_config().protocol_fee_bps, MAX_PROTOCOL_FEE_BPS);

    let e = Env::default();
    e.mock_all_auths();
    let router = SettlementRouterClient::new(&e, &e.register(SettlementRouter, ()));
    assert_eq!(
        router.try_initialize(
            &Address::generate(&e),
            &Address::generate(&e),
            &Address::generate(&e),
            &Address::generate(&e),
            &(MAX_PROTOCOL_FEE_BPS + 1),
        ),
        Err(Ok(Error::InvalidBps))
    );
}

#[test]
fn a_zero_fee_pays_the_seller_the_whole_bid() {
    let f = setup();
    f.router.set_protocol_fee_bps(&0);
    f.open_exit(1, 100 * SHARE);
    let node = f.winning_node(10_000 * USDC, 9_200 * USDC, 800);

    f.router.settle(&1, &node, &(100 * SHARE), &(9_200 * USDC));

    assert_eq!(f.usdc_bal.balance(&f.seller), 9_200 * USDC);
    assert_eq!(f.usdc_bal.balance(&f.treasury), 0);
}
