//! Tests for the milestone escrow lifecycle (#1490).
//! The `EscrowTest` fixture and role constants live in `tests/mod.rs`.

use crate::tests::{EscrowTest, DISPUTE_TIMEOUT, ESCROW_AMOUNT, PAYER_FLOAT};
use crate::{
    EscrowStatus, MicroPayContract, MicroPayContractClient, MilestoneEscrowApproved,
    MilestoneEscrowCancelled, MilestoneEscrowCreated, MilestoneEscrowDisputed, MAX_DISPUTE_TIMEOUT,
};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{token, Address, Env};

#[test]
fn test_create_milestone_escrow_locks_funds() {
    let t = EscrowTest::new();

    let id = t.fund();

    t.assert_published(MilestoneEscrowCreated {
        escrow_id: id,
        payer: t.payer.clone(),
        recipient: t.recipient.clone(),
        approver: t.approver.clone(),
        token: t.token.clone(),
        amount: ESCROW_AMOUNT,
        dispute_timeout: DISPUTE_TIMEOUT,
    });

    assert_eq!(id, 0);
    assert_eq!(t.client().get_milestone_escrow_count(), 1);
    assert_eq!(t.balance(&t.contract_id), ESCROW_AMOUNT);
    assert_eq!(t.balance(&t.payer), PAYER_FLOAT - ESCROW_AMOUNT);

    let escrow = t.escrow(&id);
    assert_eq!(escrow.status, EscrowStatus::Pending);
    assert_eq!(escrow.token, t.token);
    assert_eq!(escrow.payer, t.payer);
    assert_eq!(escrow.recipient, t.recipient);
    assert_eq!(escrow.approver, t.approver);
    assert_eq!(escrow.amount, ESCROW_AMOUNT);
    assert_eq!(escrow.dispute_ledger, 0);
    assert_eq!(escrow.dispute_timeout, DISPUTE_TIMEOUT);
}

#[test]
fn test_create_milestone_escrow_ids_increment() {
    let t = EscrowTest::new();

    assert_eq!(t.fund(), 0);
    assert_eq!(t.fund(), 1);

    assert_eq!(t.client().get_milestone_escrow_count(), 2);
    assert_eq!(t.balance(&t.contract_id), 2 * ESCROW_AMOUNT);
    assert_eq!(t.balance(&t.payer), PAYER_FLOAT - 2 * ESCROW_AMOUNT);
}

#[test]
fn test_approve_milestone_releases_funds() {
    let t = EscrowTest::new();
    let id = t.fund();

    t.approve(&id);

    t.assert_published(MilestoneEscrowApproved {
        escrow_id: id,
        approver: t.approver.clone(),
        recipient: t.recipient.clone(),
        amount: ESCROW_AMOUNT,
    });

    assert_eq!(t.balance(&t.recipient), ESCROW_AMOUNT);
    assert_eq!(t.balance(&t.contract_id), 0);
    assert_eq!(t.escrow(&id).status, EscrowStatus::Approved);
}

#[test]
fn test_approve_milestone_twice_releases_once() {
    let t = EscrowTest::new();
    let id = t.fund();
    t.approve(&id);

    let second = t.client().try_approve_milestone(&id, &t.approver);

    assert!(second.is_err());
    assert_eq!(t.balance(&t.recipient), ESCROW_AMOUNT);
}

#[test]
#[should_panic(expected = "Only the designated approver can approve this escrow")]
fn test_only_the_designated_approver_can_approve() {
    let t = EscrowTest::new();
    let id = t.fund();
    let stranger = Address::generate(&t.env);

    // `mock_all_auths` lets any address claim any signature, so the
    // recorded approver is the only thing standing between them and the funds.
    t.client().approve_milestone(&id, &stranger);
}

#[test]
#[should_panic(expected = "Escrow not found")]
fn test_unknown_escrow_id_has_no_record() {
    let t = EscrowTest::new();
    t.escrow(&7);
}

#[test]
fn test_dispute_milestone_holds_funds() {
    let t = EscrowTest::new();
    let id = t.fund();
    let disputed_at = t.sequence();

    t.dispute(&id);

    t.assert_published(MilestoneEscrowDisputed {
        escrow_id: id,
        payer: t.payer.clone(),
        amount: ESCROW_AMOUNT,
        reclaim_after: disputed_at + DISPUTE_TIMEOUT,
    });

    let escrow = t.escrow(&id);
    assert_eq!(escrow.status, EscrowStatus::Disputed);
    assert_eq!(escrow.dispute_ledger, disputed_at);
    // Neither party can move the funds while the dispute is open.
    assert_eq!(t.balance(&t.contract_id), ESCROW_AMOUNT);
    assert_eq!(t.balance(&t.recipient), 0);
    assert_eq!(t.balance(&t.payer), PAYER_FLOAT - ESCROW_AMOUNT);
}

#[test]
#[should_panic(expected = "Only the payer can dispute this escrow")]
fn test_only_the_payer_can_dispute() {
    let t = EscrowTest::new();
    let id = t.fund();
    let stranger = Address::generate(&t.env);

    t.client().dispute_milestone(&id, &stranger);
}

#[test]
#[should_panic(expected = "Escrow is no longer pending")]
fn test_disputed_escrow_cannot_be_approved() {
    let t = EscrowTest::new();
    let id = t.fund();
    t.dispute(&id);

    t.approve(&id);
}

#[test]
#[should_panic(expected = "Escrow is no longer pending")]
fn test_released_escrow_cannot_be_disputed() {
    let t = EscrowTest::new();
    let id = t.fund();
    t.approve(&id);

    t.dispute(&id);
}

#[test]
fn test_cancel_milestone_escrow_returns_funds_after_timeout() {
    let t = EscrowTest::new();
    let id = t.fund();
    t.dispute(&id);

    t.advance(DISPUTE_TIMEOUT);
    t.cancel(&id);

    t.assert_published(MilestoneEscrowCancelled {
        escrow_id: id,
        payer: t.payer.clone(),
        amount: ESCROW_AMOUNT,
    });

    assert_eq!(t.balance(&t.payer), PAYER_FLOAT);
    assert_eq!(t.balance(&t.contract_id), 0);
    assert_eq!(t.escrow(&id).status, EscrowStatus::Cancelled);
}

#[test]
#[should_panic(expected = "Escrow dispute timeout has not elapsed")]
fn test_cancel_milestone_escrow_one_ledger_early_fails() {
    let t = EscrowTest::new();
    let id = t.fund();
    t.dispute(&id);

    t.advance(DISPUTE_TIMEOUT - 1);
    t.cancel(&id);
}

#[test]
#[should_panic(expected = "Escrow is not disputed")]
fn test_cancel_pending_escrow_fails() {
    let t = EscrowTest::new();
    let id = t.fund();

    t.cancel(&id);
}

#[test]
#[should_panic(expected = "Only the payer can cancel this escrow")]
fn test_only_the_payer_can_cancel() {
    let t = EscrowTest::new();
    let id = t.fund();
    t.dispute(&id);
    t.advance(DISPUTE_TIMEOUT);

    t.client().cancel_milestone_escrow(&id, &t.approver);
}

#[test]
#[should_panic(expected = "Escrow amount must be positive")]
fn test_create_escrow_rejects_non_positive_amount() {
    let t = EscrowTest::new();
    t.client().create_milestone_escrow(
        &t.token,
        &t.payer,
        &t.recipient,
        &0,
        &t.approver,
        &DISPUTE_TIMEOUT,
    );
}

#[test]
#[should_panic(expected = "Escrow dispute timeout is out of range")]
fn test_create_escrow_rejects_zero_dispute_timeout() {
    let t = EscrowTest::new();
    t.client().create_milestone_escrow(
        &t.token,
        &t.payer,
        &t.recipient,
        &ESCROW_AMOUNT,
        &t.approver,
        &0,
    );
}

#[test]
#[should_panic(expected = "Escrow dispute timeout is out of range")]
fn test_create_escrow_rejects_timeout_beyond_the_storage_window() {
    let t = EscrowTest::new();
    t.client().create_milestone_escrow(
        &t.token,
        &t.payer,
        &t.recipient,
        &ESCROW_AMOUNT,
        &t.approver,
        &(MAX_DISPUTE_TIMEOUT + 1),
    );
}

#[test]
#[should_panic(expected = "Unauthorized function call for address")]
fn test_create_escrow_requires_the_payer_to_sign() {
    let env = Env::default();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);
    client.initialize(&Address::generate(&env));
    // Deliberately no `mock_all_auths`: the payer never authorised the call.
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let payer = Address::generate(&env);
    token::StellarAssetClient::new(&env, &token).mint(&payer, &PAYER_FLOAT);

    client.create_milestone_escrow(
        &token,
        &payer,
        &Address::generate(&env),
        &ESCROW_AMOUNT,
        &Address::generate(&env),
        &DISPUTE_TIMEOUT,
    );
}
