//! Property-style invariant tests for `close_stream` (#1085, relocated in #1490).
//! The deterministic `Lcg` PRNG helper lives in `tests/mod.rs`.

use crate::tests::{advance_by, check_close_stream_invariant, stream_fixture, Lcg};
use soroban_sdk::Env;

/// Property test verifying the streaming contract invariant:
/// `refund + claimed == deposited` — the refund returned by `close_stream`
/// plus the recipient's `claimed` total must always equal the original
/// `deposited` amount, regardless of rate, elapsed ledgers, or deposit size.
#[test]
fn test_close_stream_refund_plus_claimed_equals_deposited() {
    // ── Edge cases explicitly requested in the issue ──────────────────

    // 0 elapsed: nothing streamed, full refund.
    check_close_stream_invariant(1_000, 0, 1_000_000);

    // rate > deposit / ledger: deposit fully streamed on ledger 1,
    // refund is zero.
    check_close_stream_invariant(2_000_000, 1, 1_000_000);

    // Very large rate: rate * elapsed overflows i128, but
    // total_streamed_amount caps at `deposited` via checked_mul.
    check_close_stream_invariant(i128::MAX, 2, 1_000_000);

    // ── 100+ randomly generated (rate, elapsed, deposit) triples ────────
    let mut rng = Lcg::new(0xDEADBEEF_CAFEBABE);
    for _ in 0..100 {
        let rate = rng.gen_range(1, 1_000_001);
        let elapsed = rng.gen_u32_range(0, 50_000);
        let deposit = rng.gen_range(1, 1_000_001);
        check_close_stream_invariant(rate, elapsed, deposit);
    }
}

/// Property test with prior claims interleaved before close — the
/// invariant must still hold even if the recipient has partially claimed.
#[test]
fn test_close_stream_refund_invariant_after_partial_claims() {
    let mut rng = Lcg::new(0x12345678_9ABCDEF0);
    for _ in 0..50 {
        let rate = rng.gen_range(1, 100_001);
        let deposit = rng.gen_range(10_000, 10_000_001);
        let env = Env::default();
        let (_, client, _token_id, payer, recipient) = stream_fixture(&env, deposit);

        let stream_id = client.open_stream(&payer, &recipient, &rate, &deposit);

        // Claim at some intermediate point.
        let first_elapsed = rng.gen_u32_range(1, 100);
        advance_by(&env, first_elapsed);
        let first_claim = client.claim_stream(&stream_id, &recipient);

        // Advance further, then close.
        let remaining = rng.gen_u32_range(0, 500);
        advance_by(&env, remaining);

        // close_stream deletes the record, so read the pre-close snapshot.
        let before = client.get_stream(&stream_id);
        let total_elapsed = first_elapsed + remaining;
        let refund = client.close_stream(&stream_id, &payer);

        let vested = rate
            .saturating_mul(total_elapsed as i128)
            .max(before.claimed)
            .min(before.deposited);

        assert_eq!(
            refund + vested,
            before.deposited,
            "Invariant violated: rate={}, first={}, remaining={}, deposit={}, \
             first_claim={}, refund={}, vested={}",
            rate,
            first_elapsed,
            remaining,
            deposit,
            first_claim,
            refund,
            vested
        );
    }
}
