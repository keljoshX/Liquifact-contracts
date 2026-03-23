//! # Escrow Contract — Test Suite
//!
//! Covers the full lifecycle, edge cases, and failure conditions for
//! [`LiquifactEscrow`].
//!
//! ## Coverage map
//!
//! | Scenario                                      | Test function                        |
//! |-----------------------------------------------|--------------------------------------|
//! | Happy-path init + get                         | `test_init_and_get_escrow`           |
//! | Happy-path fund (exact target) + settle       | `test_fund_exact_and_settle`         |
//! | Partial funding (multiple investors)          | `test_partial_funding_multiple_investors` |
//! | Over-funding (funded_amount > target)         | `test_overfunding_still_funded`      |
//! | Fund after funded (status=1) panics           | `test_fund_after_funded_panics`      |
//! | Fund after settled (status=2) panics          | `test_fund_after_settled_panics`     |
//! | Settle on open escrow (status=0) panics       | `test_settle_open_panics`            |
//! | Settle twice panics                           | `test_settle_twice_panics`           |
//! | get_escrow before init panics                 | `test_get_before_init_panics`        |
//! | fund before init panics                       | `test_fund_before_init_panics`       |
//! | settle before init panics                     | `test_settle_before_init_panics`     |
//! | Init preserves all fields correctly           | `test_init_field_integrity`          |
//! | Yield bps stored and readable                 | `test_yield_bps_stored`              |
//! | Maturity stored and readable                  | `test_maturity_stored`               |
//! | Zero-amount partial fund does not flip status | `test_zero_amount_fund_no_status_change` |
//! | Single-unit funding reaches target of 1       | `test_minimum_amount_escrow`         |

use super::{LiquifactEscrow, LiquifactEscrowClient};
use soroban_sdk::{symbol_short, testutils::Address as _, Address, Env};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spin up a fresh environment, register the contract, and return the client.
fn setup() -> (Env, LiquifactEscrowClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(LiquifactEscrow, ());
    // SAFETY: the Env outlives the test function; the 'static bound is
    // satisfied by leaking the env — acceptable in test-only code.
    let env: &'static Env = Box::leak(Box::new(env));
    let client = LiquifactEscrowClient::new(env, &contract_id);
    (env.clone(), client)
}

// ---------------------------------------------------------------------------
// Happy-path tests
// ---------------------------------------------------------------------------

#[test]
fn test_init_and_get_escrow() {
    let (_, client) = setup();
    let sme = Address::generate(&Env::default());

    let escrow = client.init(
        &symbol_short!("INV001"),
        &sme,
        &10_000_0000000i128,
        &800i64,
        &1000u64,
    );

    assert_eq!(escrow.invoice_id, symbol_short!("INV001"));
    assert_eq!(escrow.amount, 10_000_0000000i128);
    assert_eq!(escrow.funding_target, 10_000_0000000i128);
    assert_eq!(escrow.funded_amount, 0);
    assert_eq!(escrow.yield_bps, 800);
    assert_eq!(escrow.maturity, 1000);
    assert_eq!(escrow.status, 0);

    // get_escrow must return identical state
    let got = client.get_escrow();
    assert_eq!(got, escrow);
}

#[test]
fn test_fund_exact_and_settle() {
    let (env, client) = setup();
    let sme = Address::generate(&env);
    let investor = Address::generate(&env);

    client.init(
        &symbol_short!("INV002"),
        &sme,
        &10_000_0000000i128,
        &800i64,
        &2000u64,
    );

    // Fund exactly the target in one shot
    let after_fund = client.fund(&investor, &10_000_0000000i128);
    assert_eq!(after_fund.funded_amount, 10_000_0000000i128);
    assert_eq!(after_fund.status, 1, "should be funded");

    // Settle
    let after_settle = client.settle();
    assert_eq!(after_settle.status, 2, "should be settled");
}

#[test]
fn test_partial_funding_multiple_investors() {
    let (env, client) = setup();
    let sme = Address::generate(&env);
    let inv_a = Address::generate(&env);
    let inv_b = Address::generate(&env);
    let inv_c = Address::generate(&env);

    client.init(
        &symbol_short!("INV003"),
        &sme,
        &9_000_0000000i128,
        &500i64,
        &3000u64,
    );

    // Three partial contributions
    let s1 = client.fund(&inv_a, &3_000_0000000i128);
    assert_eq!(s1.status, 0, "still open after first tranche");

    let s2 = client.fund(&inv_b, &3_000_0000000i128);
    assert_eq!(s2.status, 0, "still open after second tranche");

    let s3 = client.fund(&inv_c, &3_000_0000000i128);
    assert_eq!(s3.funded_amount, 9_000_0000000i128);
    assert_eq!(s3.status, 1, "funded after third tranche completes target");
}

#[test]
fn test_overfunding_still_funded() {
    let (env, client) = setup();
    let sme = Address::generate(&env);
    let investor = Address::generate(&env);

    client.init(
        &symbol_short!("INV004"),
        &sme,
        &5_000_0000000i128,
        &300i64,
        &4000u64,
    );

    // Fund more than the target
    let after = client.fund(&investor, &7_000_0000000i128);
    assert_eq!(after.funded_amount, 7_000_0000000i128);
    assert_eq!(after.status, 1, "over-funded escrow must still be status=1");
}

#[test]
fn test_init_field_integrity() {
    let (env, client) = setup();
    let sme = Address::generate(&env);

    let escrow = client.init(
        &symbol_short!("INV005"),
        &sme,
        &1_500_0000000i128,
        &1200i64,
        &9999u64,
    );

    // funding_target must mirror amount
    assert_eq!(escrow.funding_target, escrow.amount);
    // sme_address must be preserved
    assert_eq!(escrow.sme_address, sme);
}

#[test]
fn test_yield_bps_stored() {
    let (env, client) = setup();
    let sme = Address::generate(&env);

    client.init(
        &symbol_short!("INV006"),
        &sme,
        &1_000_0000000i128,
        &1500i64, // 15%
        &5000u64,
    );

    assert_eq!(client.get_escrow().yield_bps, 1500);
}

#[test]
fn test_maturity_stored() {
    let (env, client) = setup();
    let sme = Address::generate(&env);

    client.init(
        &symbol_short!("INV007"),
        &sme,
        &1_000_0000000i128,
        &800i64,
        &u64::MAX,
    );

    assert_eq!(client.get_escrow().maturity, u64::MAX);
}

#[test]
fn test_minimum_amount_escrow() {
    let (env, client) = setup();
    let sme = Address::generate(&env);
    let investor = Address::generate(&env);

    client.init(&symbol_short!("INV008"), &sme, &1i128, &0i64, &1u64);

    let after = client.fund(&investor, &1i128);
    assert_eq!(after.status, 1);

    let settled = client.settle();
    assert_eq!(settled.status, 2);
}

#[test]
fn test_zero_amount_fund_no_status_change() {
    let (env, client) = setup();
    let sme = Address::generate(&env);
    let investor = Address::generate(&env);

    client.init(
        &symbol_short!("INV009"),
        &sme,
        &1_000_0000000i128,
        &800i64,
        &1000u64,
    );

    // A zero-amount fund call should not flip status
    let after = client.fund(&investor, &0i128);
    assert_eq!(after.status, 0, "zero-amount fund must not change status");
    assert_eq!(after.funded_amount, 0);
}

// ---------------------------------------------------------------------------
// Failure / panic tests
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Escrow not open for funding")]
fn test_fund_after_funded_panics() {
    let (env, client) = setup();
    let sme = Address::generate(&env);
    let investor = Address::generate(&env);

    client.init(
        &symbol_short!("INV010"),
        &sme,
        &1_000_0000000i128,
        &800i64,
        &1000u64,
    );
    client.fund(&investor, &1_000_0000000i128); // reaches status=1
    client.fund(&investor, &1i128); // must panic
}

#[test]
#[should_panic(expected = "Escrow not open for funding")]
fn test_fund_after_settled_panics() {
    let (env, client) = setup();
    let sme = Address::generate(&env);
    let investor = Address::generate(&env);

    client.init(
        &symbol_short!("INV011"),
        &sme,
        &1_000_0000000i128,
        &800i64,
        &1000u64,
    );
    client.fund(&investor, &1_000_0000000i128);
    client.settle();
    client.fund(&investor, &1i128); // must panic
}

#[test]
#[should_panic(expected = "Escrow must be funded before settlement")]
fn test_settle_open_panics() {
    let (env, client) = setup();
    let sme = Address::generate(&env);

    client.init(
        &symbol_short!("INV012"),
        &sme,
        &1_000_0000000i128,
        &800i64,
        &1000u64,
    );
    client.settle(); // status=0, must panic
}

#[test]
#[should_panic(expected = "Escrow must be funded before settlement")]
fn test_settle_twice_panics() {
    let (env, client) = setup();
    let sme = Address::generate(&env);
    let investor = Address::generate(&env);

    client.init(
        &symbol_short!("INV013"),
        &sme,
        &1_000_0000000i128,
        &800i64,
        &1000u64,
    );
    client.fund(&investor, &1_000_0000000i128);
    client.settle();
    client.settle(); // status=2, must panic
}

#[test]
#[should_panic(expected = "Escrow not initialized")]
fn test_get_before_init_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(LiquifactEscrow, ());
    let client = LiquifactEscrowClient::new(&env, &contract_id);
    client.get_escrow();
}

#[test]
#[should_panic(expected = "Escrow not initialized")]
fn test_fund_before_init_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let investor = Address::generate(&env);
    let contract_id = env.register(LiquifactEscrow, ());
    let client = LiquifactEscrowClient::new(&env, &contract_id);
    client.fund(&investor, &1_000i128);
}

#[test]
#[should_panic(expected = "Escrow not initialized")]
fn test_settle_before_init_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(LiquifactEscrow, ());
    let client = LiquifactEscrowClient::new(&env, &contract_id);
    client.settle();
}
