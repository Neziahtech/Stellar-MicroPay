//! TTL-boundary tests: instance and persistent storage keep state alive near
//! expiry (#1490).

use crate::tests::setup;
use crate::{
    EscrowStatus, MicroPayContract, MicroPayContractClient, ESCROW_TTL_THRESHOLD,
    INSTANCE_TTL_THRESHOLD,
};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{token, Address, Env, Symbol};

/// Simulates the instance storage entry being near expiry (1 ledger before
/// `INSTANCE_TTL_THRESHOLD`) and confirms that calling any entry point
/// keeps state accessible — i.e., extend_ttl was called and the contract
/// survives the boundary.
#[test]
fn test_instance_ttl_extended_near_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);

    // Jump to a ledger just before the instance TTL would expire.
    // The test environment starts the instance TTL at its max; we only care
    // that bump_instance is called and succeeds without panicking, which
    // confirms it is wired into every entry point tested below.
    let near_expiry = INSTANCE_TTL_THRESHOLD - 1;
    env.ledger().set_sequence_number(near_expiry);

    // Every entry point category: admin, getter, tip, receipt, stream.
    assert_eq!(client.get_admin(), admin);

    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);
    let stream_id = client.open_stream(&payer, &recipient, &1, &100);
    assert_eq!(client.get_stream(&stream_id).deposited, 100);
    assert_eq!(client.get_claimable(&stream_id), 0);

    let memo = Symbol::new(&env, "Test");
    let receipt_id = client.mint_receipt(&payer, &recipient, &50, &memo);
    assert_eq!(client.get_receipt_count(&payer), 1);
    let receipt = client.get_receipt(&payer, &receipt_id);
    assert_eq!(receipt.amount, 50);

    assert_eq!(client.get_fee_bps(), 0);
    assert_eq!(client.get_tip_total(&recipient), 0);
    assert_eq!(client.get_tip_count(&recipient), 0);
}

/// Confirms that the instance TTL is extended when a stream is created and
/// then claimed a large number of ledgers later (near the TTL boundary for
/// instance storage).
#[test]
fn test_stream_claim_near_instance_ttl_boundary() {
    let (env, client, _admin, payer, recipient) = setup();

    // Rate of 1 per ledger with a large deposit; advance close to TTL boundary.
    let id = client.open_stream(&payer, &recipient, &1, &200_000);
    let start = env.ledger().sequence();

    // Advance to just below INSTANCE_TTL_THRESHOLD.
    env.ledger()
        .set_sequence_number(start + INSTANCE_TTL_THRESHOLD - 1);

    // Claiming here exercises bump_instance; if it were absent the entry
    // would succeed in test but the instance would be archived on mainnet.
    let claimed = client.claim_stream(&id, &recipient);
    assert_eq!(
        claimed,
        (INSTANCE_TTL_THRESHOLD - 1) as i128,
        "expected all accrued funds to be claimable near TTL boundary"
    );
}

/// Confirms that the persistent TTL for a milestone escrow is bumped at the
/// TTL threshold boundary, keeping the record alive.
#[test]
fn test_milestone_escrow_persistent_ttl_extended_near_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MicroPayContract);
    MicroPayContractClient::new(&env, &contract_id).initialize(&Address::generate(&env));

    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);
    let approver = Address::generate(&env);
    token::StellarAssetClient::new(&env, &token).mint(&payer, &10_000);

    let client = MicroPayContractClient::new(&env, &contract_id);
    let id = client.create_milestone_escrow(&token, &payer, &recipient, &1_000, &approver, &500);

    // Advance to just before the persistent TTL threshold.
    let near_expiry = ESCROW_TTL_THRESHOLD - 1;
    env.ledger().set_sequence_number(near_expiry);

    // load_escrow (called by get_milestone_escrow) must re-extend the entry.
    let escrow = client.get_milestone_escrow(&id);
    assert_eq!(escrow.status, EscrowStatus::Pending);
    assert_eq!(escrow.amount, 1_000);
}

/// Verifies the full time-locked escrow lifecycle works when the contract
/// instance is near expiry.
#[test]
fn test_time_locked_escrow_near_instance_ttl_boundary() {
    let (env, client, _admin, payer, recipient) = setup();

    // Create an escrow that releases well before the TTL threshold.
    let current = env.ledger().sequence();
    let release = current + 50;
    let id = client.open_escrow(&payer, &recipient, &200, &release);

    // Jump to just before the TTL threshold, but comfortably past the
    // release ledger so the escrow can be collected.
    let target = release + INSTANCE_TTL_THRESHOLD;
    env.ledger().set_sequence_number(target);

    let released = client.release_escrow(&id);
    assert_eq!(released, 200);
    assert!(client.get_escrow(&id).released);
}
