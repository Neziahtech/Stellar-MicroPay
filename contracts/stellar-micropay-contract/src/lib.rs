#![no_std]

/**
 * contracts/stellar-micropay-contract/src/lib.rs
 *
 * Stellar MicroPay — Soroban Smart Contract
 *
 * Provides:
 *   - Streaming payments (open/claim/top-up/close + pause/resume)
 *   - Escrow payments (ROADMAP v2.1)
 *   - Milestone-based escrow release with dispute timeout (ROADMAP v2.1)
 *   - Creator tipping (ROADMAP v1.4)
 *   - Micro-transaction batching (ROADMAP v2.0)
 *   - NFT payment receipts (ROADMAP v1.5)
 *
 * Module layout (#1490): the implementation is split per feature module and
 * this file keeps the single exported contract interface. Every entry point
 * below is a thin delegation to its feature module, so the compiled contract
 * spec is unchanged:
 *   - `storage.rs`  — storage keys, TTL constants, shared guards
 *   - `admin.rs`    — initialization, upgrade, admin rotation, freeze
 *   - `tips.rs`     — tipping and the operator fee
 *   - `receipts.rs` — NFT payment receipts
 *   - `escrow.rs`   — time-locked escrow and milestone escrow
 *   - `streams.rs`  — streaming payment channels
 *   - `batch.rs`    — batch-send placeholder (ROADMAP v2.0)
 *   - `tests/`      — test suites split per feature
 */
mod batch;
mod storage;

// soroban-sdk 29 deprecated the tuple-style `env.events().publish(...)` in
// favour of `#[contractevent]` types and renamed `update_current_contract_wasm`
// to `update_current_contract`. Migrating them would change the on-chain event
// shapes and admin upgrade path, which is out of scope for the module split
// (#1490); #1493's clippy gate therefore allows the deprecated calls here
// explicitly, per module, instead of repo-wide. Track the migration separately.
#[allow(deprecated)]
mod admin;
#[allow(deprecated)]
mod escrow;
#[allow(deprecated)]
mod receipts;
#[allow(deprecated)]
mod streams;
#[allow(deprecated)]
mod tips;

#[cfg(test)]
#[allow(deprecated)] // `Env::register_contract` -> `register` rename in SDK 29
mod tests;

// Test suites print diagnostics with `std::println!`; `no_std` otherwise.
#[cfg(test)]
extern crate std;

use soroban_sdk::{contract, contracterror, contractimpl, Address, BytesN, Env, Symbol};

// Re-exported so the crate surface matches the previous single-file layout.
pub use escrow::{
    EscrowRecord, EscrowStatus, MilestoneEscrowApproved, MilestoneEscrowCancelled,
    MilestoneEscrowCreated, MilestoneEscrowDisputed, MilestoneEscrowRecord,
};
pub use receipts::ReceiptMetadata;
pub use storage::DataKey;
pub use streams::Stream;
pub use tips::{TipEventData, TipRecord};

// Crate-internal re-exports consumed only by the test suites.
#[cfg(test)]
pub(crate) use escrow::MAX_DISPUTE_TIMEOUT;
#[cfg(test)]
pub(crate) use storage::{ESCROW_TTL_THRESHOLD, INSTANCE_TTL_THRESHOLD};

// ─── Contract errors ──────────────────────────────────────────────────────────

/// Typed errors surfaced to callers via Soroban's generated client.
/// Discriminants are stable — do not reorder or remove variants.
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContractError {
    /// The caller is not the stored admin.
    NotAdmin = 1,
    /// The contract is frozen; state-changing calls are rejected.
    /// Read-only getters keep working while frozen.
    Frozen = 2,
}

// ─── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct MicroPayContract;

/// Exported contract interface. Every function below is a one-line delegation
/// to the feature module that holds the implementation; names, signatures,
/// argument order, and doc comments define the contract spec and must not
/// drift from the original single-file layout.
#[contractimpl]
impl MicroPayContract {
    // ─── Admin (`admin.rs`) ──────────────────────────────────────────────────

    /// Initialize the contract with an admin address.
    /// Can only be called once.
    ///
    /// Security review (#1121):
    ///   - `admin` must authorize, so the stored admin address can never be
    ///     set to a value that the admin itself did not sign off on.
    ///   - Once written, the admin can never be changed: the duplicate-init
    ///     check rejects any later call before it reaches the write.
    ///
    /// Residual risk (documented, not changed by this review): `initialize`
    /// is a separate invocation from the deploy, so the first caller can
    /// still install *themselves* as admin — `require_auth` proves the stored
    /// admin consented to being admin, it cannot prove the address is the
    /// deployer. Deploy via a `__constructor` (deploy-time init) or invoke
    /// `initialize` in the same transaction as the deploy to close that
    /// window entirely.
    pub fn initialize(env: Env, admin: Address) {
        admin::initialize(env, admin)
    }

    /// Upgrade the WASM code of the current contract.
    /// Admin-gated: only stored Admin address can call this function.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        admin::upgrade(env, new_wasm_hash)
    }

    /// Rotate/update the contract admin address.
    /// Admin-gated: only current Admin address can set a new admin.
    pub fn set_admin(env: Env, new_admin: Address) {
        admin::set_admin(env, new_admin)
    }

    /// Pause all state-changing operations until [`unfreeze`] is called.
    /// Read-only getters and admin functions remain available while frozen so
    /// monitoring and recovery operations can still inspect and repair state.
    /// Admin-gated.
    pub fn freeze(env: Env, admin: Address) {
        admin::freeze(env, admin)
    }

    /// Clear the freeze flag, re-enabling state-changing operations.
    /// Admin-gated.
    pub fn unfreeze(env: Env, admin: Address) {
        admin::unfreeze(env, admin)
    }

    /// Whether the contract is currently frozen. Always callable.
    pub fn is_frozen(env: Env) -> bool {
        admin::is_frozen(env)
    }

    /// Get the contract admin address.
    ///
    /// Security review (#1121): read-only accessor — `get_admin` writes no
    /// state, so it needs no authorization. It reveals the admin address,
    /// which is already public on-chain.
    pub fn get_admin(env: Env) -> Address {
        admin::get_admin(env)
    }

    // ─── Tipping (`tips.rs`) ─────────────────────────────────────────────────

    /// Send a tip from `from` to `to` using a Stellar token.
    ///
    /// Parameters:
    ///   - token_address: The SAC (Stellar Asset Contract) address for the token (e.g. XLM)
    ///   - from:          The sender (must authorize this call)
    ///   - to:            The recipient
    ///   - amount:        Amount in the token's smallest unit (stroops for XLM)
    ///
    /// If an operator fee is configured (see `set_fee_bps`), `amount * fee_bps / 10000`
    /// is transferred to the admin and the remainder to `to`. This records the tip
    /// on-chain for analytics and emits an event.
    pub fn send_tip(env: Env, token_address: Address, from: Address, to: Address, amount: i128) {
        tips::send_tip(env, token_address, from, to, amount)
    }

    /// Send several tips atomically. The sender is debited once for the total;
    /// the contract then fans the funds out to each unique recipient.
    pub fn batch_tip(
        env: Env,
        token_address: Address,
        from: Address,
        tips: soroban_sdk::Vec<(Address, i128)>,
    ) {
        tips::batch_tip(env, token_address, from, tips)
    }

    /// Set the operator fee charged on every tip, in basis points
    /// (1 bps = 0.01%). Capped at 500 bps (5%). Only callable by the
    /// current admin. A `fee_bps` of 0 disables the fee (the default).
    pub fn set_fee_bps(env: Env, admin: Address, fee_bps: u32) {
        tips::set_fee_bps(env, admin, fee_bps)
    }

    /// Get the currently configured operator fee, in basis points.
    pub fn get_fee_bps(env: Env) -> u32 {
        tips::get_fee_bps(env)
    }

    /// Get the total amount tipped to a recipient (in stroops).
    ///
    /// Security review (#1121): read-only accessor — no `require_auth` is
    /// required because no state is written and the value is public.
    pub fn get_tip_total(env: Env, recipient: Address) -> i128 {
        tips::get_tip_total(env, recipient)
    }

    /// Get the number of tips received by a recipient.
    ///
    /// Security review (#1121): read-only accessor — no `require_auth` is
    /// required because no state is written and the value is public.
    pub fn get_tip_count(env: Env, recipient: Address) -> u32 {
        tips::get_tip_count(env, recipient)
    }

    /// Get a specific tip record for a recipient by index.
    ///
    /// Security review (#1121): read-only accessor — no `require_auth` is
    /// required because no state is written. Tip records contain only public
    /// payment data.
    pub fn get_tip_record(env: Env, recipient: Address, index: u32) -> TipRecord {
        tips::get_tip_record(env, recipient, index)
    }

    // ─── NFT Receipts (`receipts.rs`) ────────────────────────────────────────

    /// Mint an on-chain receipt as proof of payment.
    pub fn mint_receipt(env: Env, from: Address, to: Address, amount: i128, memo: Symbol) -> u32 {
        receipts::mint_receipt(env, from, to, amount, memo)
    }

    /// Get the total number of receipts minted for a payer.
    ///
    /// Security review (#1121): read-only accessor — no `require_auth` is
    /// required because no state is written and the count is public.
    pub fn get_receipt_count(env: Env, payer: Address) -> u32 {
        receipts::get_receipt_count(env, payer)
    }

    /// Get a specific receipt for a payer by index.
    ///
    /// Security review (#1121): read-only accessor — no `require_auth` is
    /// required because no state is written. Receipt metadata contains only
    /// public payment data.
    pub fn get_receipt(env: Env, payer: Address, index: u32) -> ReceiptMetadata {
        receipts::get_receipt(env, payer, index)
    }

    // ─── Milestone escrow (`escrow.rs`) ──────────────────────────────────────

    /// Lock `amount` of `token` in the contract until a third party confirms
    /// the milestone was met.
    ///
    /// The payer funds the escrow up front. `approver` alone can release it
    /// with [`approve_milestone`]; the payer alone can freeze it with
    /// [`dispute_milestone`] and reclaim it through [`cancel_milestone_escrow`] once
    /// `dispute_timeout` ledgers have passed.
    ///
    /// Returns the escrow id used by every other escrow call.
    pub fn create_milestone_escrow(
        env: Env,
        token: Address,
        payer: Address,
        recipient: Address,
        amount: i128,
        approver: Address,
        dispute_timeout: u32,
    ) -> u32 {
        escrow::create_milestone_escrow(
            env,
            token,
            payer,
            recipient,
            amount,
            approver,
            dispute_timeout,
        )
    }

    /// Release the escrowed funds to the recipient because the milestone was met.
    ///
    /// Callable only by the escrow's `approver`, and only while the escrow is
    /// still pending — a disputed escrow is frozen until the payer reclaims it.
    pub fn approve_milestone(env: Env, escrow_id: u32, approver: Address) {
        escrow::approve_milestone(env, escrow_id, approver)
    }

    /// Freeze a pending escrow because the payer contests the milestone.
    ///
    /// The funds stay in the contract; after `dispute_timeout` ledgers the
    /// payer may reclaim them with [`cancel_milestone_escrow`].
    pub fn dispute_milestone(env: Env, escrow_id: u32, payer: Address) {
        escrow::dispute_milestone(env, escrow_id, payer)
    }

    /// Reclaim a disputed escrow once its dispute timeout has elapsed.
    ///
    /// The funds go back to the payer and the escrow is closed.
    ///
    /// Named `cancel_milestone_escrow` rather than `cancel_escrow` because the
    /// contract already exposes `cancel_escrow` for time-locked escrows.
    pub fn cancel_milestone_escrow(env: Env, escrow_id: u32, payer: Address) {
        escrow::cancel_milestone_escrow(env, escrow_id, payer)
    }

    /// Get a milestone escrow record by id.
    pub fn get_milestone_escrow(env: Env, escrow_id: u32) -> MilestoneEscrowRecord {
        escrow::get_milestone_escrow(env, escrow_id)
    }

    /// Get the number of milestone escrows created so far.
    pub fn get_milestone_escrow_count(env: Env) -> u32 {
        escrow::get_milestone_escrow_count(env)
    }

    // ─── Streaming (`streams.rs`) ────────────────────────────────────────────

    /// Open a new payment stream. Returns the stream id.
    pub fn open_stream(
        env: Env,
        payer: Address,
        recipient: Address,
        rate_per_ledger: i128,
        deposit: i128,
    ) -> u32 {
        streams::open_stream(env, payer, recipient, rate_per_ledger, deposit)
    }

    /// Claim all currently claimable funds for a stream.
    pub fn claim_stream(env: Env, stream_id: u32, recipient: Address) -> i128 {
        streams::claim_stream(env, stream_id, recipient)
    }

    /// Add more funds to an existing stream, extending its duration.
    pub fn top_up_stream(env: Env, stream_id: u32, payer: Address, amount: i128) {
        streams::top_up_stream(env, stream_id, payer, amount)
    }

    /// Close a stream; payer is refunded the unstreamed remainder.
    ///
    /// **Mathematical invariant:** after `close_stream` returns,
    /// `refund + claimed == deposited` holds by construction — the recipient
    /// receives every streamable stroop (`claimed` rises to `total_streamed`),
    /// and the payer receives the balance (`refund = deposited - claimed`).
    ///
    /// Returns the refund amount.
    pub fn close_stream(env: Env, stream_id: u32, payer: Address) -> i128 {
        streams::close_stream(env, stream_id, payer)
    }

    /// Pause a stream: freezes elapsed time at the current ledger.
    pub fn pause_stream(env: Env, stream_id: u32, payer: Address) {
        streams::pause_stream(env, stream_id, payer)
    }

    /// Resume a paused stream: shifts start_ledger forward by the paused duration.
    pub fn resume_stream(env: Env, stream_id: u32, payer: Address) {
        streams::resume_stream(env, stream_id, payer)
    }

    /// Get a stream record.
    pub fn get_stream(env: Env, stream_id: u32) -> Stream {
        streams::get_stream(env, stream_id)
    }

    /// Get the currently claimable amount for a stream.
    pub fn get_claimable(env: Env, stream_id: u32) -> i128 {
        streams::get_claimable(env, stream_id)
    }

    // ─── Time-locked escrow (`escrow.rs`) ────────────────────────────────────

    /// Lock funds until `release_ledger`. Returns the escrow id.
    pub fn open_escrow(
        env: Env,
        payer: Address,
        recipient: Address,
        amount: i128,
        release_ledger: u32,
    ) -> u32 {
        escrow::open_escrow(env, payer, recipient, amount, release_ledger)
    }

    /// Release escrow funds to the recipient after `release_ledger`.
    /// Anyone can call once the ledger threshold is reached.
    pub fn release_escrow(env: Env, escrow_id: u32) -> i128 {
        escrow::release_escrow(env, escrow_id)
    }

    /// Cancel an escrow before `release_ledger`; funds return to the payer.
    pub fn cancel_escrow(env: Env, escrow_id: u32, payer: Address) -> i128 {
        escrow::cancel_escrow(env, escrow_id, payer)
    }

    /// Get an escrow record.
    pub fn get_escrow(env: Env, escrow_id: u32) -> EscrowRecord {
        escrow::get_escrow(env, escrow_id)
    }

    // ─── Placeholders (`batch.rs`) ───────────────────────────────────────────

    /// [PLACEHOLDER] Batch multiple micro-payments in a single transaction.
    /// See ROADMAP.md v2.0 — Multi-Currency Payments.
    ///
    /// Security review (#1121): the stub panics before touching any state, so
    /// it requires no authorization today. The real implementation must
    /// `require_auth` from `from` (the single payer for the whole batch), and
    /// must validate every amount before requesting authorization.
    pub fn batch_send(
        env: Env,
        _from: Address,
        _recipients: soroban_sdk::Vec<Address>,
        _amounts: soroban_sdk::Vec<i128>,
    ) {
        batch::batch_send(env, _from, _recipients, _amounts)
    }
}
