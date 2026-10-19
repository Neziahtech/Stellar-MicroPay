//! Tests for NFT payment receipts (#1490).

use crate::{MicroPayContract, MicroPayContractClient};
use soroban_sdk::testutils::{
    Address as _, AuthorizedFunction, AuthorizedInvocation, MockAuth, MockAuthInvoke,
};
use soroban_sdk::{Address, Env, IntoVal, Symbol};

#[test]
fn test_mint_receipt_stores_receipt_record_accessible_via_get_receipt() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_all_auths();
    client.initialize(&admin);

    let payer = Address::generate(&env);
    let payee = Address::generate(&env);

    env.mock_all_auths();

    let memo = Symbol::new(&env, "Rent");
    let receipt_id = client.mint_receipt(&payer, &payee, &1000, &memo);
    assert_eq!(receipt_id, 0);

    assert_eq!(client.get_receipt_count(&payer), 1);

    let stored = client.get_receipt(&payer, &0);
    assert_eq!(stored.from, payer);
    assert_eq!(stored.to, payee);
    assert_eq!(stored.amount, 1000);
    assert_eq!(stored.memo, memo);
    assert_eq!(stored.ledger, env.ledger().sequence());
}

#[test]
fn test_mint_two_receipts_from_same_payer_increments_count() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_all_auths();
    client.initialize(&admin);

    let payer = Address::generate(&env);
    let payee1 = Address::generate(&env);
    let payee2 = Address::generate(&env);

    let id1 = client.mint_receipt(&payer, &payee1, &500, &Symbol::new(&env, "Coffee"));
    let id2 = client.mint_receipt(&payer, &payee2, &1500, &Symbol::new(&env, "Invoice"));

    assert_eq!(id1, 0);
    assert_eq!(id2, 1);
    assert_eq!(client.get_receipt_count(&payer), 2);

    let receipt1 = client.get_receipt(&payer, &0);
    assert_eq!(receipt1.to, payee1);
    assert_eq!(receipt1.amount, 500);

    let receipt2 = client.get_receipt(&payer, &1);
    assert_eq!(receipt2.to, payee2);
    assert_eq!(receipt2.amount, 1500);
}

#[test]
fn test_mint_receipt_requires_from_auth() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_all_auths();
    client.initialize(&admin);

    let from = Address::generate(&env);
    let to = Address::generate(&env);

    client.mint_receipt(&from, &to, &1000, &Symbol::new(&env, "Rent"));

    // Exactly one auth entry, owned by the payer, covering the full
    // `mint_receipt` invocation arguments.
    assert_eq!(
        env.auths(),
        [(
            from.clone(),
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    client.address.clone(),
                    Symbol::new(&env, "mint_receipt"),
                    (
                        from.clone(),
                        to.clone(),
                        1000_i128,
                        Symbol::new(&env, "Rent")
                    )
                        .into_val(&env)
                )),
                sub_invocations: [].into(),
            }
        )]
    );
}

/// Issue #1121 acceptance criteria: only the payer can mint their own
/// receipt — an unrelated authorized address must not mint one on their
/// behalf, and no receipt state may be written.
#[test]
fn test_mint_receipt_rejects_unauthorized_caller() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "initialize",
            args: (&admin,).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.initialize(&admin);

    let from = Address::generate(&env);
    let to = Address::generate(&env);
    let attacker = Address::generate(&env);

    // Only `attacker` is authorized; the receipt belongs to `from`.
    env.mock_auths(&[MockAuth {
        address: &attacker,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "mint_receipt",
            args: (
                from.clone(),
                to.clone(),
                1000_i128,
                Symbol::new(&env, "Rent"),
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);

    let result = client.try_mint_receipt(&from, &to, &1000, &Symbol::new(&env, "Rent"));
    assert!(result.is_err());
    assert_eq!(client.get_receipt_count(&from), 0);
}

/// Regression test for the auth-ordering fix: a call that panics on
/// validation must not leave behind a satisfied authorization for the
/// payer (which would be usable as a probe inside a bigger transaction).
#[test]
fn test_failed_mint_does_not_consume_auth() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_all_auths();
    client.initialize(&admin);

    let from = Address::generate(&env);
    let to = Address::generate(&env);

    // Amount validation happens *before* require_auth, so this panics
    // without registering any authorization for `from`.
    let result = client.try_mint_receipt(&from, &to, &0, &Symbol::new(&env, "Bad"));
    assert!(result.is_err());
    assert!(env.auths().is_empty());
    assert_eq!(client.get_receipt_count(&from), 0);

    // A valid call afterwards still works.
    let id = client.mint_receipt(&from, &to, &250, &Symbol::new(&env, "Good"));
    assert_eq!(id, 0);
    assert_eq!(client.get_receipt_count(&from), 1);
}

#[test]
#[should_panic(expected = "Receipt not found")]
fn test_get_receipt_out_of_range_panics_gracefully() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_all_auths();
    client.initialize(&admin);

    let payer = Address::generate(&env);
    client.get_receipt(&payer, &0); // No receipts minted yet -> panics
}
