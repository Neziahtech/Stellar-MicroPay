//! Tests for tipping and batch tips (#1490).

use crate::{MicroPayContract, MicroPayContractClient};
use soroban_sdk::testutils::{Address as _, AuthorizedFunction, MockAuth, MockAuthInvoke};
use soroban_sdk::{token, Address, Env, IntoVal, Symbol, TryFromVal};

/// End-to-end tip flow against a real (test) Stellar Asset Contract:
/// verifies the transfer executes and that the sender is the authorizer
/// of the `send_tip` invocation.
#[test]
fn test_send_tip_moves_funds_and_requires_from_auth() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_all_auths();
    client.initialize(&admin);

    // Register a native-like SAC and fund the tip sender.
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let sac_address = sac.address();
    let token = token::Client::new(&env, &sac_address);
    let sac_admin = token::StellarAssetClient::new(&env, &sac_address);

    let from = Address::generate(&env);
    let to = Address::generate(&env);
    let amount = 123_i128;
    sac_admin.mint(&from, &10_000);

    client.send_tip(&sac_address, &from, &to, &amount);

    // Exactly one auth entry is recorded and it belongs to the sender
    // at the `send_tip` root (the SAC transfer appears nested under it).
    // NOTE: `env.auths()` only reflects the most recent top-level
    // invocation, so this must run before any further client calls.
    let auths = env.auths();
    assert_eq!(auths.len(), 1);
    let (authorizer, invocation) = &auths[0];
    assert_eq!(authorizer, &from);
    match &invocation.function {
        AuthorizedFunction::Contract((contract_address, name, args)) => {
            assert_eq!(contract_address, &client.address);
            assert_eq!(name, &Symbol::new(&env, "send_tip"));
            assert_eq!(args.len(), 4);
            // (token, from, to, amount) — sender and amount must be
            // covered by the authorization to prevent argument swapping.
            assert_eq!(
                Address::try_from_val(&env, &args.get(1).unwrap()).unwrap(),
                from
            );
            assert_eq!(
                Address::try_from_val(&env, &args.get(2).unwrap()).unwrap(),
                to
            );
            assert_eq!(
                i128::try_from_val(&env, &args.get(3).unwrap()).unwrap(),
                amount
            );
        }
        _ => panic!("expected contract function authorization"),
    }

    // The transfer actually happened.
    assert_eq!(token.balance(&from), 10_000 - amount);
    assert_eq!(token.balance(&to), amount);
    assert_eq!(client.get_tip_total(&to), amount);
    assert_eq!(client.get_tip_count(&to), 1);
}

#[test]
#[should_panic(expected = "Tip amount must be positive")]
fn test_send_tip_rejects_zero_amount() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_all_auths();
    client.initialize(&admin);

    let from = Address::generate(&env);
    let to = Address::generate(&env);
    client.send_tip(&Address::generate(&env), &from, &to, &0);
}

#[test]
#[should_panic(expected = "Tip amount must be positive")]
fn test_send_tip_rejects_negative_amount() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_all_auths();
    client.initialize(&admin);

    let from = Address::generate(&env);
    let to = Address::generate(&env);
    client.send_tip(&Address::generate(&env), &from, &to, &-5);
}

#[test]
fn test_send_tip_rejects_unauthorized_caller() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    // Initialize by mocking only the admin's authorization.
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

    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let sac_address = sac.address();
    let token = token::Client::new(&env, &sac_address);
    let sac_admin = token::StellarAssetClient::new(&env, &sac_address);

    let from = Address::generate(&env);
    let to = Address::generate(&env);

    // Fund the sender: the SAC only mints for the admin that created it.
    env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &sac_address,
            fn_name: "mint",
            args: (&from, &10_000_i128).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    sac_admin.mint(&from, &10_000);
    assert_eq!(token.balance(&from), 10_000);

    // Only `attacker` is authorized for `send_tip`; `from` never signed.
    let attacker = Address::generate(&env);
    env.mock_auths(&[MockAuth {
        address: &attacker,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "send_tip",
            args: (sac_address.clone(), from.clone(), to.clone(), 100_i128).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    let result = client.try_send_tip(&sac_address, &from, &to, &100);
    assert!(result.is_err());

    // Unauthorized call moved no funds and recorded no tip.
    assert_eq!(token.balance(&from), 10_000);
    assert_eq!(token.balance(&to), 0);
    assert_eq!(client.get_tip_total(&to), 0);
    assert_eq!(client.get_tip_count(&to), 0);
}

#[test]
fn test_tip_totals_start_at_zero() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_all_auths();
    client.initialize(&admin);

    let recipient = Address::generate(&env);
    assert_eq!(client.get_tip_total(&recipient), 0);
    assert_eq!(client.get_tip_count(&recipient), 0);
}

fn tip_setup() -> (Env, MicroPayContractClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    let asset = env.register_stellar_asset_contract_v2(issuer);
    let token = asset.address();
    let sender = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&sender, &10_000);
    (env, client, token, sender)
}

#[test]
fn test_batch_tip_single_and_five_recipients() {
    let (env, client, token_address, sender) = tip_setup();
    let one = Address::generate(&env);
    let one_tip = soroban_sdk::Vec::from_array(&env, [(one.clone(), 100i128)]);
    client.batch_tip(&token_address, &sender, &one_tip);
    assert_eq!(client.get_tip_total(&one), 100);
    assert_eq!(client.get_tip_count(&one), 1);

    let mut five = soroban_sdk::Vec::new(&env);
    for amount in 1..=5i128 {
        five.push_back((Address::generate(&env), amount));
    }
    client.batch_tip(&token_address, &sender, &five);
    for i in 0..five.len() {
        let (recipient, amount) = five.get(i).unwrap();
        assert_eq!(client.get_tip_total(&recipient), amount);
        assert_eq!(client.get_tip_count(&recipient), 1);
    }
}

#[test]
#[should_panic(expected = "Tip amount must be positive")]
fn test_batch_tip_rejects_zero_amount() {
    let (env, client, token_address, sender) = tip_setup();
    let tips = soroban_sdk::Vec::from_array(&env, [(Address::generate(&env), 0i128)]);
    client.batch_tip(&token_address, &sender, &tips);
}

#[test]
#[should_panic(expected = "Duplicate tip recipient")]
fn test_batch_tip_rejects_duplicate_recipient() {
    let (env, client, token_address, sender) = tip_setup();
    let recipient = Address::generate(&env);
    let tips =
        soroban_sdk::Vec::from_array(&env, [(recipient.clone(), 100i128), (recipient, 200i128)]);
    client.batch_tip(&token_address, &sender, &tips);
}
