//! Tests for the time-locked escrow lifecycle (#1490).

use crate::tests::setup;
use soroban_sdk::testutils::Ledger;

#[test]
fn test_open_escrow() {
    let (env, client, _admin, payer, recipient) = setup();
    let release = env.ledger().sequence() + 100;
    let id = client.open_escrow(&payer, &recipient, &500, &release);
    assert_eq!(id, 0);
    let record = client.get_escrow(&0);
    assert_eq!(record.payer, payer);
    assert_eq!(record.recipient, recipient);
    assert_eq!(record.amount, 500);
    assert_eq!(record.release_ledger, release);
    assert!(!record.released);
    assert!(!record.cancelled);
}

#[test]
fn test_release_escrow_after_release_ledger() {
    let (env, client, _admin, payer, recipient) = setup();
    let release = env.ledger().sequence() + 10;
    let id = client.open_escrow(&payer, &recipient, &500, &release);
    env.ledger().set_sequence_number(release + 1);
    let amount = client.release_escrow(&id);
    assert_eq!(amount, 500);
    assert!(client.get_escrow(&id).released);
}

#[test]
#[should_panic(expected = "Escrow is still locked")]
fn test_release_escrow_before_release_ledger_fails() {
    let (env, client, _admin, payer, recipient) = setup();
    let release = env.ledger().sequence() + 100;
    let id = client.open_escrow(&payer, &recipient, &500, &release);
    env.ledger().set_sequence_number(release - 10);
    client.release_escrow(&id);
}

#[test]
fn test_cancel_escrow_before_release() {
    let (env, client, _admin, payer, recipient) = setup();
    let release = env.ledger().sequence() + 100;
    let id = client.open_escrow(&payer, &recipient, &500, &release);
    let amount = client.cancel_escrow(&id, &payer);
    assert_eq!(amount, 500);
    assert!(client.get_escrow(&id).cancelled);
}

#[test]
#[should_panic(expected = "Escrow lock has expired")]
fn test_cancel_escrow_after_release_fails() {
    let (env, client, _admin, payer, recipient) = setup();
    let release = env.ledger().sequence() + 10;
    let id = client.open_escrow(&payer, &recipient, &500, &release);
    env.ledger().set_sequence_number(release + 1);
    client.cancel_escrow(&id, &payer);
}

#[test]
fn test_escrow_ids_increment() {
    let (env, client, _admin, payer, recipient) = setup();
    let release = env.ledger().sequence() + 50;
    let id0 = client.open_escrow(&payer, &recipient, &100, &release);
    let id1 = client.open_escrow(&payer, &recipient, &200, &release);
    assert_eq!(id0, 0);
    assert_eq!(id1, 1);
    assert_eq!(client.get_escrow(&id1).amount, 200);
}
