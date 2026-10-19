//! Tests for admin controls: initialization, rotation, upgrade, freeze (#1490).

use crate::tests::setup;
use crate::{MicroPayContract, MicroPayContractClient};
use soroban_sdk::testutils::{
    Address as _, AuthorizedFunction, AuthorizedInvocation, MockAuth, MockAuthInvoke,
};
use soroban_sdk::{token, Address, BytesN, Env, IntoVal, Symbol};

#[test]
fn test_initialize() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_all_auths();

    client.initialize(&admin);

    assert_eq!(client.get_admin(), admin);
}

#[test]
fn test_initialize_requires_admin_auth() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_all_auths();

    client.initialize(&admin);

    // The admin must be the (only) authorizer of `initialize`.
    assert_eq!(
        env.auths(),
        [(
            admin.clone(),
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    client.address.clone(),
                    Symbol::new(&env, "initialize"),
                    (admin.clone(),).into_val(&env)
                )),
                sub_invocations: [].into(),
            }
        )]
    );
}

#[test]
#[should_panic(expected = "Contract already initialized")]
fn test_double_initialize_fails() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_all_auths();
    client.initialize(&admin);
    client.initialize(&admin); // should panic
}

/// Issue #1121 acceptance criteria: an entry point must fail when the
/// caller provides no authorization at all.
#[test]
fn test_initialize_rejects_unauthorized_admin() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);

    // No authorization is mocked, so `admin.require_auth()` must fail.
    let result = client.try_initialize(&admin);
    assert!(result.is_err());

    // Nothing was written: the contract is still uninitialized.
    assert!(client.try_get_admin().is_err());
}

/// Issue #1121 acceptance criteria: no operation may let an unauthorized
/// address act as another — authorizing `attacker` must not install
/// `victim` as admin.
#[test]
fn test_initialize_rejects_authorization_of_another_address() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let victim = Address::generate(&env);
    let attacker = Address::generate(&env);

    // Only `attacker` signs, while the admin being stored is `victim`.
    env.mock_auths(&[MockAuth {
        address: &attacker,
        invoke: &MockAuthInvoke {
            contract: &contract_id,
            fn_name: "initialize",
            args: (&victim,).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    let result = client.try_initialize(&victim);
    assert!(result.is_err());
    assert!(client.try_get_admin().is_err());
}

#[test]
fn test_set_admin_and_rotation() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin);
    assert_eq!(client.get_admin(), admin);

    let new_admin = Address::generate(&env);
    client.set_admin(&new_admin);

    assert_eq!(client.get_admin(), new_admin);
}

#[test]
#[should_panic]
fn test_upgrade_non_admin_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin);

    env.set_auths(&[]);
    let dummy_hash = BytesN::from_array(&env, &[1u8; 32]);
    // Without admin auth, calling upgrade panics
    client.upgrade(&dummy_hash);
}

#[test]
fn test_upgrade_admin_requires_auth_and_invokes_deployer() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let dummy_hash = BytesN::from_array(&env, &[1u8; 32]);
    // With admin auth, try_upgrade passes the admin auth check and invokes deployer.
    // In native test environment, deployer returns an Err (InvalidAction for non-wasm target).
    let res = client.try_upgrade(&dummy_hash);
    assert!(res.is_err());
}

#[test]
fn test_upgrade_preserves_stream_and_tip_state() {
    let env = Env::default();
    env.mock_all_auths();

    const STUB_WASM: &[u8] = include_bytes!("../../test_stub.wasm");
    let contract_id = env.register_contract_wasm(None, STUB_WASM);
    let client = MicroPayContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin);

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin.clone())
        .address();
    let token_client = token::StellarAssetClient::new(&env, &token_address);

    let payer1 = Address::generate(&env);
    let payer2 = Address::generate(&env);
    let recipient1 = Address::generate(&env);
    let recipient2 = Address::generate(&env);

    token_client.mint(&payer1, &100_000);
    token_client.mint(&payer2, &100_000);

    // 1. Create 2 streams
    let stream_id_0 = client.open_stream(&payer1, &recipient1, &10, &1000);
    let stream_id_1 = client.open_stream(&payer2, &recipient2, &20, &2000);

    // 2. Create 3 tips
    client.send_tip(&token_address, &payer1, &recipient1, &150);
    client.send_tip(&token_address, &payer2, &recipient1, &250);
    client.send_tip(&token_address, &payer1, &recipient2, &350);

    // Query before state
    let stream_0_before = client.get_stream(&stream_id_0);
    let stream_1_before = client.get_stream(&stream_id_1);
    let tip_total_1_before = client.get_tip_total(&recipient1);
    let tip_total_2_before = client.get_tip_total(&recipient2);

    std::println!("--- BEFORE UPGRADE STATE ---");
    std::println!("Stream 0: {:?}", stream_0_before);
    std::println!("Stream 1: {:?}", stream_1_before);
    std::println!("Tip Total Recipient 1: {}", tip_total_1_before);
    std::println!("Tip Total Recipient 2: {}", tip_total_2_before);

    // 3. Upgrade to mock new WASM hash using test stub WASM
    let new_wasm_hash = env.deployer().upload_contract_wasm(STUB_WASM);
    client.upgrade(&new_wasm_hash);

    // 4. Query after state
    let stream_0_after = client.get_stream(&stream_id_0);
    let stream_1_after = client.get_stream(&stream_id_1);
    let tip_total_1_after = client.get_tip_total(&recipient1);
    let tip_total_2_after = client.get_tip_total(&recipient2);

    std::println!("--- AFTER UPGRADE STATE ---");
    std::println!("Stream 0: {:?}", stream_0_after);
    std::println!("Stream 1: {:?}", stream_1_after);
    std::println!("Tip Total Recipient 1: {}", tip_total_1_after);
    std::println!("Tip Total Recipient 2: {}", tip_total_2_after);

    // 5. Verify state is preserved
    assert_eq!(stream_0_before.payer, stream_0_after.payer);
    assert_eq!(stream_0_before.recipient, stream_0_after.recipient);
    assert_eq!(
        stream_0_before.rate_per_ledger,
        stream_0_after.rate_per_ledger
    );
    assert_eq!(stream_0_before.deposited, stream_0_after.deposited);
    assert_eq!(stream_0_before.claimed, stream_0_after.claimed);

    assert_eq!(stream_1_before.payer, stream_1_after.payer);
    assert_eq!(stream_1_before.recipient, stream_1_after.recipient);
    assert_eq!(
        stream_1_before.rate_per_ledger,
        stream_1_after.rate_per_ledger
    );
    assert_eq!(stream_1_before.deposited, stream_1_after.deposited);
    assert_eq!(stream_1_before.claimed, stream_1_after.claimed);

    assert_eq!(tip_total_1_before, tip_total_1_after);
    assert_eq!(tip_total_2_before, tip_total_2_after);
    assert_eq!(tip_total_1_after, 400);
    assert_eq!(tip_total_2_after, 350);
}

#[test]
fn test_freeze_blocks_send_tip_and_mint_receipt() {
    let (env, client, admin, payer, recipient) = setup();
    assert!(!client.is_frozen());

    client.freeze(&admin);
    assert!(client.is_frozen());

    // send_tip must be rejected while frozen.
    let issuer = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(issuer).address();
    let res = client.try_send_tip(&token, &payer, &recipient, &100);
    assert!(res.is_err());

    // mint_receipt must also be rejected.
    let res2 = client.try_mint_receipt(&payer, &recipient, &50, &Symbol::new(&env, "Test"));
    assert!(res2.is_err());
}

#[test]
fn test_unfreeze_restores_operations() {
    let (_env, client, admin, _payer, _recipient) = setup();
    client.freeze(&admin);
    assert!(client.is_frozen());

    client.unfreeze(&admin);
    assert!(!client.is_frozen());

    // get_admin still works while frozen and after unfreeze.
    assert_eq!(client.get_admin(), admin);
    // is_frozen (read-only) always works.
    assert!(!client.is_frozen());
}

#[test]
#[should_panic]
fn test_freeze_requires_admin() {
    let (env, client, _admin, _payer, _recipient) = setup();
    let non_admin = Address::generate(&env);
    client.freeze(&non_admin);
}

#[test]
fn test_read_only_getters_work_while_frozen() {
    let (_env, client, admin, _payer, recipient) = setup();
    client.freeze(&admin);
    // These must not panic while the contract is frozen.
    let _ = client.get_admin();
    let _ = client.get_fee_bps();
    let _ = client.get_tip_total(&recipient);
    let _ = client.get_tip_count(&recipient);
    let _ = client.is_frozen();
}
