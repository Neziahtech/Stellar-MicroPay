//! Streaming payment channels: open/claim/top-up/close plus pause/resume
//! (#1490).

use crate::storage::{bump_instance, DataKey};
use soroban_sdk::{contracttype, Address, Env, Symbol};

/// A streaming payment channel: payer deposits funds streamed at a fixed rate.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Stream {
    pub payer: Address,
    pub recipient: Address,
    pub rate_per_ledger: i128,
    pub deposited: i128,
    pub claimed: i128,
    pub start_ledger: u32,
    /// Ledger at which the stream was paused (None = running).
    pub pause_ledger: Option<u32>,
}

// ─── Streaming ─────────────────────────────────────────────────────────

pub(crate) fn open_stream(
    env: Env,
    payer: Address,
    recipient: Address,
    rate_per_ledger: i128,
    deposit: i128,
) -> u32 {
    bump_instance(&env);
    payer.require_auth();

    if rate_per_ledger <= 0 {
        panic!("Rate must be positive");
    }
    if deposit <= 0 {
        panic!("Deposit must be positive");
    }

    let count: u32 = env
        .storage()
        .instance()
        .get(&DataKey::StreamCount)
        .unwrap_or(0);

    let stream = Stream {
        payer: payer.clone(),
        recipient: recipient.clone(),
        rate_per_ledger,
        deposited: deposit,
        claimed: 0,
        start_ledger: env.ledger().sequence(),
        pause_ledger: None,
    };

    env.storage()
        .instance()
        .set(&DataKey::Stream(count), &stream);
    env.storage()
        .instance()
        .set(&DataKey::StreamCount, &(count + 1));

    env.events().publish(
        (
            Symbol::new(&env, "MicroPayContract"),
            Symbol::new(&env, "open_stream"),
            payer,
            recipient,
        ),
        count,
    );

    count
}

pub(crate) fn claim_stream(env: Env, stream_id: u32, recipient: Address) -> i128 {
    bump_instance(&env);
    recipient.require_auth();

    let mut stream: Stream = env
        .storage()
        .instance()
        .get(&DataKey::Stream(stream_id))
        .expect("Stream not found");

    if stream.recipient != recipient {
        panic!("Not stream recipient");
    }

    let current_ledger = env.ledger().sequence();
    let elapsed_ledgers = current_ledger.saturating_sub(stream.start_ledger);
    let total_streamed = stream
        .rate_per_ledger
        .checked_mul(elapsed_ledgers as i128)
        .unwrap_or(stream.deposited)
        .min(stream.deposited);
    let claimable = total_streamed - stream.claimed;

    // Freeze accrual while paused: recompute against pause_ledger.
    let claimable = if let Some(paused_at) = stream.pause_ledger {
        let paused_elapsed = paused_at.saturating_sub(stream.start_ledger);
        let paused_total = stream
            .rate_per_ledger
            .checked_mul(paused_elapsed as i128)
            .unwrap_or(stream.deposited)
            .min(stream.deposited);
        paused_total - stream.claimed
    } else {
        claimable
    };

    // Cap by remaining deposit.
    let remaining = stream.deposited - stream.claimed;
    let mut payout = claimable.max(0).min(remaining);
    if payout < 0 {
        payout = 0;
    }

    stream.claimed += payout;
    env.storage()
        .instance()
        .set(&DataKey::Stream(stream_id), &stream);

    env.events().publish(
        (
            Symbol::new(&env, "MicroPayContract"),
            Symbol::new(&env, "claim_stream"),
            recipient,
            stream_id,
        ),
        payout,
    );

    payout
}

pub(crate) fn top_up_stream(env: Env, stream_id: u32, payer: Address, amount: i128) {
    bump_instance(&env);
    payer.require_auth();

    if amount <= 0 {
        panic!("Top-up amount must be positive");
    }

    let mut stream: Stream = env
        .storage()
        .instance()
        .get(&DataKey::Stream(stream_id))
        .expect("Stream not found");

    if stream.payer != payer {
        panic!("Not stream payer");
    }

    stream.deposited += amount;
    env.storage()
        .instance()
        .set(&DataKey::Stream(stream_id), &stream);

    env.events().publish(
        (
            Symbol::new(&env, "MicroPayContract"),
            Symbol::new(&env, "top_up_stream"),
            payer,
            stream_id,
        ),
        amount,
    );
}

pub(crate) fn close_stream(env: Env, stream_id: u32, payer: Address) -> i128 {
    bump_instance(&env);
    payer.require_auth();

    let mut stream: Stream = env
        .storage()
        .instance()
        .get(&DataKey::Stream(stream_id))
        .expect("Stream not found");

    if stream.payer != payer {
        panic!("Not stream payer");
    }

    let current_ledger = env.ledger().sequence();
    // Effective ledger is frozen at pause point while paused.
    let effective = stream.pause_ledger.unwrap_or(current_ledger);
    let elapsed = effective.saturating_sub(stream.start_ledger);
    let total_streamed = stream
        .rate_per_ledger
        .checked_mul(elapsed as i128)
        .unwrap_or(stream.deposited)
        .min(stream.deposited);

    // After close the recipient has effectively "claimed" everything that
    // was ever streamable. The `Stream` record carries no token field, so
    // this is a bookkeeping update rather than a transfer; the on-chain
    // payout path lives in `claim_stream`.
    stream.claimed = total_streamed;

    // Refund = what was deposited minus everything the recipient now holds.
    let refund = stream.deposited - stream.claimed;

    env.storage().instance().remove(&DataKey::Stream(stream_id));

    env.events().publish(
        (
            Symbol::new(&env, "MicroPayContract"),
            Symbol::new(&env, "close_stream"),
            payer,
            stream_id,
        ),
        refund,
    );

    refund
}

pub(crate) fn pause_stream(env: Env, stream_id: u32, payer: Address) {
    bump_instance(&env);
    payer.require_auth();

    let mut stream: Stream = env
        .storage()
        .instance()
        .get(&DataKey::Stream(stream_id))
        .expect("Stream not found");

    if stream.payer != payer {
        panic!("Not stream payer");
    }
    if stream.pause_ledger.is_some() {
        panic!("Stream already paused");
    }

    stream.pause_ledger = Some(env.ledger().sequence());
    env.storage()
        .instance()
        .set(&DataKey::Stream(stream_id), &stream);

    env.events().publish(
        (
            Symbol::new(&env, "MicroPayContract"),
            Symbol::new(&env, "pause_stream"),
            payer,
            stream_id,
        ),
        env.ledger().sequence(),
    );
}

pub(crate) fn resume_stream(env: Env, stream_id: u32, payer: Address) {
    bump_instance(&env);
    payer.require_auth();

    let mut stream: Stream = env
        .storage()
        .instance()
        .get(&DataKey::Stream(stream_id))
        .expect("Stream not found");

    if stream.payer != payer {
        panic!("Not stream payer");
    }

    let paused_at = stream.pause_ledger.expect("Stream is not paused");
    let current_ledger = env.ledger().sequence();
    let paused_duration = current_ledger.saturating_sub(paused_at);
    stream.start_ledger = stream.start_ledger.saturating_add(paused_duration);
    stream.pause_ledger = None;
    env.storage()
        .instance()
        .set(&DataKey::Stream(stream_id), &stream);

    env.events().publish(
        (
            Symbol::new(&env, "MicroPayContract"),
            Symbol::new(&env, "resume_stream"),
            payer,
            stream_id,
        ),
        current_ledger,
    );
}

pub(crate) fn get_stream(env: Env, stream_id: u32) -> Stream {
    bump_instance(&env);
    env.storage()
        .instance()
        .get(&DataKey::Stream(stream_id))
        .expect("Stream not found")
}

pub(crate) fn get_claimable(env: Env, stream_id: u32) -> i128 {
    bump_instance(&env);
    let stream: Stream = env
        .storage()
        .instance()
        .get(&DataKey::Stream(stream_id))
        .expect("Stream not found");

    let current_ledger = env.ledger().sequence();
    let elapsed_ledgers = current_ledger.saturating_sub(stream.start_ledger);
    let total_streamed = stream.rate_per_ledger * elapsed_ledgers as i128;
    let claimable = total_streamed - stream.claimed;

    // While paused, accrual is frozen at pause_ledger.
    let claimable = if let Some(paused_at) = stream.pause_ledger {
        let paused_elapsed = paused_at.saturating_sub(stream.start_ledger);
        let paused_total = stream.rate_per_ledger * paused_elapsed as i128;
        paused_total - stream.claimed
    } else {
        claimable
    };

    let remaining = stream.deposited - stream.claimed;
    claimable.max(0).min(remaining)
}
