#![cfg(test)]
extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

use crate::{Error, TestnetFaucet, TestnetFaucetClient, DEFAULT_COOLDOWN};

const DRIP: i128 = 10_000_0000000; // 10,000 USDC at 7 decimals

struct Fixture<'a> {
    e: Env,
    faucet: TestnetFaucetClient<'a>,
    token: Address,
    bal: token::Client<'a>,
    admin: Address,
}

fn setup() -> Fixture<'static> {
    let e = Env::default();
    e.mock_all_auths();
    e.ledger().set_timestamp(1_000_000);

    let admin = Address::generate(&e);
    let issuer = Address::generate(&e);
    let sac = e.register_stellar_asset_contract_v2(issuer.clone());
    let token = sac.address();

    let faucet = TestnetFaucetClient::new(&e, &e.register(TestnetFaucet, ()));
    faucet.initialize(&admin, &DEFAULT_COOLDOWN);

    // The whole point: the faucet holds the mint, not a person.
    token::StellarAssetClient::new(&e, &token).set_admin(&faucet.address);
    faucet.set_drip(&token, &DRIP);

    Fixture {
        bal: token::Client::new(&e, &token),
        e,
        faucet,
        token,
        admin,
    }
}

#[test]
fn claims_the_drip() {
    let f = setup();
    let user = Address::generate(&f.e);

    assert_eq!(f.faucet.claim(&user, &f.token), DRIP);
    assert_eq!(f.bal.balance(&user), DRIP);
}

#[test]
fn a_first_claim_is_never_gated() {
    // `last == 0` must read as "never claimed", not as a claim at the epoch.
    let f = setup();
    f.e.ledger().set_timestamp(DEFAULT_COOLDOWN - 1);
    let user = Address::generate(&f.e);
    assert_eq!(f.faucet.claim(&user, &f.token), DRIP);
}

#[test]
fn a_second_claim_waits_out_the_cooldown() {
    let f = setup();
    let user = Address::generate(&f.e);
    f.faucet.claim(&user, &f.token);

    assert_eq!(
        f.faucet.try_claim(&user, &f.token),
        Err(Ok(Error::TooSoon.into()))
    );

    f.e.ledger()
        .set_timestamp(1_000_000 + DEFAULT_COOLDOWN);
    assert_eq!(f.faucet.claim(&user, &f.token), DRIP);
    assert_eq!(f.bal.balance(&user), DRIP * 2);
}

#[test]
fn the_cooldown_is_per_person() {
    let f = setup();
    let a = Address::generate(&f.e);
    let b = Address::generate(&f.e);

    f.faucet.claim(&a, &f.token);
    // b has never claimed; a's cooldown is not b's problem.
    assert_eq!(f.faucet.claim(&b, &f.token), DRIP);
}

#[test]
fn an_unregistered_token_is_refused() {
    let f = setup();
    let user = Address::generate(&f.e);
    let issuer = Address::generate(&f.e);
    let other = f.e.register_stellar_asset_contract_v2(issuer).address();

    assert_eq!(
        f.faucet.try_claim(&user, &other),
        Err(Ok(Error::UnknownToken.into()))
    );
}

#[test]
fn a_zero_drip_closes_the_tap() {
    let f = setup();
    let user = Address::generate(&f.e);
    f.faucet.set_drip(&f.token, &0);

    assert_eq!(
        f.faucet.try_claim(&user, &f.token),
        Err(Ok(Error::UnknownToken.into()))
    );
}

#[test]
fn next_claim_reports_the_wait() {
    let f = setup();
    let user = Address::generate(&f.e);

    assert_eq!(f.faucet.next_claim(&user, &f.token), 0);
    f.faucet.claim(&user, &f.token);
    assert_eq!(
        f.faucet.next_claim(&user, &f.token),
        1_000_000 + DEFAULT_COOLDOWN
    );

    f.e.ledger()
        .set_timestamp(1_000_000 + DEFAULT_COOLDOWN);
    assert_eq!(f.faucet.next_claim(&user, &f.token), 0);
}

#[test]
fn tokens_lists_the_tap() {
    let f = setup();
    assert_eq!(f.faucet.tokens().len(), 1);
    assert_eq!(f.faucet.tokens().get(0).unwrap(), f.token);

    // Re-setting the same token must not list it twice.
    f.faucet.set_drip(&f.token, &(DRIP * 2));
    assert_eq!(f.faucet.tokens().len(), 1);
    assert_eq!(f.faucet.drip(&f.token), DRIP * 2);
}

#[test]
fn admin_can_hand_the_mint_back() {
    let f = setup();
    f.faucet.set_token_admin(&f.token, &f.admin);

    // The faucet no longer holds the mint, so its own claim path dies inside
    // the token rather than quietly minting.
    let user = Address::generate(&f.e);
    assert!(f.faucet.try_claim(&user, &f.token).is_err());
}

#[test]
fn a_negative_drip_is_refused() {
    let f = setup();
    assert_eq!(
        f.faucet.try_set_drip(&f.token, &-1),
        Err(Ok(Error::InvalidAmount.into()))
    );
}

#[test]
fn it_initializes_once() {
    let f = setup();
    assert_eq!(
        f.faucet.try_initialize(&f.admin, &DEFAULT_COOLDOWN),
        Err(Ok(Error::AlreadyInitialized.into()))
    );
}
