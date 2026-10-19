//! Shared test fixtures and helpers for the contract test suites (#1490).
//!
//! Each feature has its own test module beside the module it exercises:
//!   - `admin.rs`             — initialization, rotation, upgrade, freeze
//!   - `tips.rs`              — tipping and batch tips
//!   - `receipts.rs`          — NFT payment receipts
//!   - `streams.rs`           — streaming payment channels
//!   - `stream_properties.rs` — randomized `close_stream` invariants (#1085)
//!   - `escrow.rs`            — time-locked escrow lifecycle
//!   - `milestone.rs`         — milestone escrow lifecycle
//!   - `escrow_properties.rs` — randomized escrow-conservation invariants (#1487)
//!   - `ttl.rs`               — TTL-boundary behaviour
//!
//! `extern crate std` lives at the crate root (see `lib.rs`) so every test
//! module can use `std::` paths.

mod admin;
mod escrow;
mod escrow_properties;
mod milestone;
mod receipts;
mod stream_properties;
mod streams;
mod tips;
mod ttl;

use crate::{MicroPayContract, MicroPayContractClient, MilestoneEscrowRecord};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger},
    token, Address, Env,
};

pub(super) fn setup() -> (
    Env,
    MicroPayContractClient<'static>,
    Address,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let payer = Address::generate(&env);
    let recipient = Address::generate(&env);
    client.initialize(&admin);
    (env, client, admin, payer, recipient)
}

pub(super) fn advance_by(env: &Env, ledgers: u32) {
    env.ledger().with_mut(|info| {
        info.sequence_number = info.sequence_number.saturating_add(ledgers);
    });
}

pub(super) fn stream_fixture(
    env: &Env,
    funding: i128,
) -> (
    Address,
    MicroPayContractClient<'_>,
    Address,
    Address,
    Address,
) {
    env.mock_all_auths();
    let contract_id = env.register_contract(None, MicroPayContract);
    let client = MicroPayContractClient::new(env, &contract_id);
    let admin = Address::generate(env);
    client.initialize(&admin);

    let payer = Address::generate(env);
    let recipient = Address::generate(env);
    let token_id = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    token::StellarAssetClient::new(env, &token_id).mint(&payer, &funding);

    (contract_id, client, token_id, payer, recipient)
}

// ─── Deterministic PRNG used by the randomized stream tests ─────────────────

/// Deterministic linear congruential generator — no external crate needed.
pub(super) struct Lcg(u64);

impl Lcg {
    pub(super) fn new(seed: u64) -> Self {
        Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }

    pub(super) fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    pub(super) fn gen_range(&mut self, min: i128, max: i128) -> i128 {
        let span = max - min;
        min + (self.next_u64() as i128 % span)
    }

    pub(super) fn gen_u32_range(&mut self, min: u32, max: u32) -> u32 {
        let span = (max - min) as u64;
        min + (self.next_u64() % span) as u32
    }
}

/// Helper that opens a stream, advances the ledger, closes it, then asserts
/// the core mathematical invariant: `refund + claimed == deposited`.
pub(super) fn check_close_stream_invariant(rate: i128, elapsed: u32, deposit: i128) {
    let env = Env::default();
    let (_, client, _token_id, payer, recipient) = stream_fixture(&env, deposit);

    let stream_id = client.open_stream(&payer, &recipient, &rate, &deposit);

    if elapsed > 0 {
        advance_by(&env, elapsed);
    }

    // close_stream deletes the Stream record, so snapshot it beforehand;
    // the post-close `claimed` total is derived from the contract's own
    // rule instead of read back from storage.
    let before = client.get_stream(&stream_id);
    let refund = client.close_stream(&stream_id, &payer);

    let vested = rate
        .saturating_mul(elapsed as i128)
        .max(before.claimed)
        .min(before.deposited);

    assert_eq!(
        refund + vested,
        before.deposited,
        "Invariant violated: rate={}, elapsed={}, deposit={}, refund={}, vested={}",
        rate,
        elapsed,
        deposit,
        refund,
        vested
    );
}

// ─── Milestone escrow fixtures (shared by milestone + property tests) ───────

/// Ledger gap the payer must wait out after disputing a milestone.
pub(super) const DISPUTE_TIMEOUT: u32 = 500;
/// Amount locked by every escrow created in these tests.
pub(super) const ESCROW_AMOUNT: i128 = 4_000;
/// Balance the payer starts every escrow test with.
pub(super) const PAYER_FLOAT: i128 = 10_000;

/// A deployed contract, a test token, and the three escrow roles.
///
/// `env.events().all()` only reports the events of the most recent
/// invocation, so read published events before balances or records, which
/// are themselves contract calls.
pub(super) struct EscrowTest {
    pub env: Env,
    pub contract_id: Address,
    pub token: Address,
    pub payer: Address,
    pub recipient: Address,
    pub approver: Address,
}

impl EscrowTest {
    pub(super) fn new() -> Self {
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
        token::StellarAssetClient::new(&env, &token).mint(&payer, &PAYER_FLOAT);

        Self {
            env,
            contract_id,
            token,
            payer,
            recipient,
            approver,
        }
    }

    pub(super) fn client(&self) -> MicroPayContractClient<'_> {
        MicroPayContractClient::new(&self.env, &self.contract_id)
    }

    pub(super) fn balance(&self, holder: &Address) -> i128 {
        token::Client::new(&self.env, &self.token).balance(holder)
    }

    pub(super) fn escrow(&self, id: &u32) -> MilestoneEscrowRecord {
        self.client().get_milestone_escrow(id)
    }

    pub(super) fn fund(&self) -> u32 {
        self.client().create_milestone_escrow(
            &self.token,
            &self.payer,
            &self.recipient,
            &ESCROW_AMOUNT,
            &self.approver,
            &DISPUTE_TIMEOUT,
        )
    }

    pub(super) fn approve(&self, id: &u32) {
        self.client().approve_milestone(id, &self.approver)
    }

    pub(super) fn dispute(&self, id: &u32) {
        self.client().dispute_milestone(id, &self.payer)
    }

    pub(super) fn cancel(&self, id: &u32) {
        self.client().cancel_milestone_escrow(id, &self.payer)
    }

    pub(super) fn sequence(&self) -> u32 {
        self.env.ledger().sequence()
    }

    pub(super) fn advance(&self, ledgers: u32) {
        let next = self.env.ledger().sequence() + ledgers;
        self.env.ledger().set_sequence_number(next);
    }

    pub(super) fn assert_published<E: soroban_sdk::events::Event>(&self, event: E) {
        let expected = event.to_xdr(&self.env, &self.contract_id);
        assert!(
            self.env.events().all().events().contains(&expected),
            "expected event was not published"
        );
    }
}
