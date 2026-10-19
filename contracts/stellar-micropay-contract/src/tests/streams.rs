//! Tests for streaming payment channels (#1490).

use crate::tests::{advance_by, setup, stream_fixture};
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{Address, Env};

#[test]
fn test_open_stream() {
    let (env, client, _admin, payer, recipient) = setup();
    let id = client.open_stream(&payer, &recipient, &10, &1000);
    assert_eq!(id, 0);
    let stream = client.get_stream(&0);
    assert_eq!(stream.payer, payer);
    assert_eq!(stream.recipient, recipient);
    assert_eq!(stream.rate_per_ledger, 10);
    assert_eq!(stream.deposited, 1000);
    assert_eq!(stream.claimed, 0);
    let _ = env;
}

#[test]
fn test_claim_stream_basic() {
    let (env, client, _admin, payer, recipient) = setup();
    let id = client.open_stream(&payer, &recipient, &10, &1000);
    let stream = client.get_stream(&id);
    env.ledger().set_sequence_number(stream.start_ledger + 10);
    let claimed = client.claim_stream(&id, &recipient);
    assert_eq!(claimed, 100);
}

#[test]
fn test_claim_stream_multiple_times() {
    let (env, client, _admin, payer, recipient) = setup();
    let id = client.open_stream(&payer, &recipient, &5, &1000);
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + 4);
    let first = client.claim_stream(&id, &recipient);
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + 4);
    let second = client.claim_stream(&id, &recipient);
    assert_eq!(first, 20);
    assert_eq!(second, 20);
}

#[test]
fn test_claim_stream_exceeds_deposit() {
    let (env, client, _admin, payer, recipient) = setup();
    let id = client.open_stream(&payer, &recipient, &100, &150);
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + 10);
    let claimed = client.claim_stream(&id, &recipient);
    assert_eq!(claimed, 150);
    // Second claim yields nothing.
    let claimed2 = client.claim_stream(&id, &recipient);
    assert_eq!(claimed2, 0);
}

#[test]
fn test_top_up_stream() {
    let (_env, client, _admin, payer, recipient) = setup();
    let id = client.open_stream(&payer, &recipient, &10, &100);
    client.top_up_stream(&id, &payer, &500);
    let stream = client.get_stream(&id);
    assert_eq!(stream.deposited, 600);
}

#[test]
fn test_close_stream_with_refund() {
    let (env, client, _admin, payer, recipient) = setup();
    let id = client.open_stream(&payer, &recipient, &10, &1000);
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + 5);
    let refund = client.close_stream(&id, &payer);
    assert_eq!(refund, 950);
}

#[test]
fn test_close_stream_after_claims() {
    let env = Env::default();
    let (_, client, _token_id, payer, recipient) = stream_fixture(&env, 100_000);

    let rate: i128 = 100;
    let deposit: i128 = 100_000;
    let id = client.open_stream(&payer, &recipient, &rate, &deposit);
    advance_by(&env, 30);
    client.claim_stream(&id, &recipient);
    advance_by(&env, 20);

    let refund = client.close_stream(&id, &payer);
    let streamed = rate * 50;
    assert_eq!(refund, deposit - streamed);
    assert!(client.try_get_stream(&id).is_err());
}

#[test]
fn test_get_claimable() {
    let (env, client, _admin, payer, recipient) = setup();
    let id = client.open_stream(&payer, &recipient, &10, &1000);
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + 3);
    assert_eq!(client.get_claimable(&id), 30);
}

#[test]
#[should_panic(expected = "Stream not found")]
fn test_claim_nonexistent_stream() {
    let (_env, client, _admin, _payer, recipient) = setup();
    client.claim_stream(&999, &recipient);
}

#[test]
#[should_panic(expected = "Not stream recipient")]
fn test_unauthorized_claim() {
    let (env, client, _admin, payer, recipient) = setup();
    let id = client.open_stream(&payer, &recipient, &10, &1000);
    let impostor = Address::generate(&env);
    // Auth is mocked, but the contract-level recipient check must reject.
    client.claim_stream(&id, &impostor);
}

#[test]
#[should_panic(expected = "Not stream payer")]
fn test_unauthorized_close() {
    let (env, client, _admin, payer, recipient) = setup();
    let id = client.open_stream(&payer, &recipient, &10, &1000);
    let impostor = Address::generate(&env);
    client.close_stream(&id, &impostor);
}

#[test]
#[should_panic(expected = "Rate must be positive")]
fn test_invalid_rate() {
    let (_env, client, _admin, payer, recipient) = setup();
    client.open_stream(&payer, &recipient, &0, &100);
}

#[test]
#[should_panic(expected = "Deposit must be positive")]
fn test_invalid_deposit() {
    let (_env, client, _admin, payer, recipient) = setup();
    client.open_stream(&payer, &recipient, &10, &0);
}

#[test]
fn test_pause_resume_claim_flow() {
    let (env, client, _admin, payer, recipient) = setup();
    let start = env.ledger().sequence();
    let id = client.open_stream(&payer, &recipient, &10, &10000);

    // Advance 5 ledgers, then pause.
    env.ledger().set_sequence_number(start + 5);
    client.pause_stream(&id, &payer);
    assert_eq!(client.get_claimable(&id), 50);

    // Advance 10 more ledgers while paused: claimable must NOT increase.
    env.ledger().set_sequence_number(start + 15);
    assert_eq!(client.get_claimable(&id), 50);
    let paused_claim = client.claim_stream(&id, &recipient);
    assert_eq!(paused_claim, 50);

    // Resume and advance 5 ledgers: claimable SHOULD increase again.
    client.resume_stream(&id, &payer);
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + 5);
    let after = client.get_claimable(&id);
    assert_eq!(after, 50);
    let resumed_claim = client.claim_stream(&id, &recipient);
    assert_eq!(resumed_claim, 50);
}
