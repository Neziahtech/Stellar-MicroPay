//! Storage keys, TTL constants, and shared storage helpers (#1490).
//!
//! Everything in this module is infrastructure used by more than one feature
//! module; feature-specific keys live with their feature's module.

use crate::ContractError;
use soroban_sdk::{contracttype, Address, Env};

/// Storage keys.
#[contracttype]
pub enum DataKey {
    Admin,
    TipTotal(Address),
    TipCount(Address),
    /// Latest tip record for a recipient (indexed by recipient + count)
    TipRecord(Address, u32),
    /// Total receipt count for a payer
    ReceiptCount(Address),
    /// Receipt record indexed by (payer, index)
    ReceiptRecord(Address, u32),
    /// Operator fee, in basis points, charged on every tip
    FeeBps,
    /// Total number of streams ever opened
    StreamCount,
    /// Stream record indexed by stream id
    Stream(u32),
    /// Total number of escrows ever opened
    EscrowCount,
    /// Escrow record indexed by escrow id
    Escrow(u32),
    /// Milestone escrow record, indexed by escrow id
    MilestoneEscrow(u32),
    /// Number of milestone escrows ever created (the next escrow id)
    MilestoneEscrowCount,
    /// Global circuit-breaker: while true, state-changing calls are rejected.
    Frozen,
}

// ─── TTL constants ────────────────────────────────────────────────────────────

/// Extend the contract instance TTL when the remaining lifetime drops below
/// this many ledgers (~14 days at 5 s/ledger on mainnet).
pub(crate) const INSTANCE_TTL_THRESHOLD: u32 = 100_000;

/// Target lifetime to set on the instance entry when it is extended
/// (~28 days at 5 s/ledger). Must not exceed the network's max entry lifetime.
pub(crate) const INSTANCE_TTL_BUMP: u32 = 200_000;

/// Escrow records live in persistent storage and are re-extended on every read
/// and write, so a record cannot be evicted while it still holds funds.
/// `ESCROW_TTL_BUMP` has to stay below the network's maximum entry lifetime.
pub(crate) const ESCROW_TTL_THRESHOLD: u32 = 100_000;
pub(crate) const ESCROW_TTL_BUMP: u32 = 200_000;

// ─── Shared helpers ───────────────────────────────────────────────────────────

/// Bump the contract instance TTL at the start of every entry point so the
/// instance storage entry is never archived while the contract is actively
/// used. The bump is a no-op when the remaining TTL already exceeds
/// `INSTANCE_TTL_THRESHOLD`.
#[inline]
pub(crate) fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_BUMP);
}

/// Reject the call if the contract is frozen.
/// Read-only getters deliberately do **not** call this — a frozen contract
/// must still be inspectable by monitoring and UI.
#[inline]
pub(crate) fn require_not_frozen(env: &Env) {
    let frozen: bool = env
        .storage()
        .instance()
        .get(&DataKey::Frozen)
        .unwrap_or(false);
    if frozen {
        env.panic_with_error(ContractError::Frozen);
    }
}
