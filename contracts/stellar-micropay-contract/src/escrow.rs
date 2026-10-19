//! Both escrow systems (#1490):
//!
//!   - Time-locked escrow (`open_escrow` / `release_escrow` / `cancel_escrow`)
//!     — bookkeeping records that release once a ledger threshold is reached.
//!   - Milestone escrow (`create_milestone_escrow` / `approve_milestone` /
//!     `dispute_milestone` / `cancel_milestone_escrow`) — funds actually held
//!     by the contract, released by a designated approver or reclaimed by the
//!     payer after a dispute times out.

use crate::storage::{bump_instance, DataKey, ESCROW_TTL_BUMP, ESCROW_TTL_THRESHOLD};
use soroban_sdk::{contractevent, contracttype, token, Address, Env, Symbol};

/// Upper bound on `dispute_timeout`. It must stay below
/// `ESCROW_TTL_THRESHOLD` so the record outlives the dispute window it was
/// created with.
pub(crate) const MAX_DISPUTE_TIMEOUT: u32 = 50_000;

/// An escrow payment locked until a release ledger.
#[contracttype]
#[derive(Clone, Debug)]
pub struct EscrowRecord {
    pub payer: Address,
    pub recipient: Address,
    pub amount: i128,
    pub release_ledger: u32,
    pub released: bool,
    pub cancelled: bool,
}

/// Lifecycle of a milestone escrow.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EscrowStatus {
    /// Funds are locked, awaiting approval or dispute.
    Pending,
    /// The approver released the funds to the recipient.
    Approved,
    /// The payer disputed the milestone; funds are held until the timeout.
    Disputed,
    /// The payer reclaimed the funds after the dispute timeout elapsed.
    Cancelled,
}

/// Funds locked in the contract until a milestone is approved or a dispute
/// times out.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneEscrowRecord {
    /// SAC of the token the funds are held in
    pub token: Address,
    /// Who locked the funds and may reclaim them
    pub payer: Address,
    /// Who receives the funds when the milestone is approved
    pub recipient: Address,
    /// The third party whose approval releases the funds
    pub approver: Address,
    /// Amount in the token's smallest unit (stroops for XLM)
    pub amount: i128,
    pub status: EscrowStatus,
    /// Ledger the escrow was created in
    pub created_ledger: u32,
    /// Ledger the dispute was raised in; `0` while undisputed
    pub dispute_ledger: u32,
    /// Ledgers the payer must wait after disputing before reclaiming
    pub dispute_timeout: u32,
}

/// Emitted when funds are locked into a milestone escrow.
#[contractevent(topics = ["milestone_escrow", "created"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneEscrowCreated {
    #[topic]
    pub escrow_id: u32,
    #[topic]
    pub payer: Address,
    pub recipient: Address,
    pub approver: Address,
    pub token: Address,
    pub amount: i128,
    pub dispute_timeout: u32,
}

/// Emitted when the approver releases the funds to the recipient.
#[contractevent(topics = ["milestone_escrow", "approved"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneEscrowApproved {
    #[topic]
    pub escrow_id: u32,
    #[topic]
    pub approver: Address,
    pub recipient: Address,
    pub amount: i128,
}

/// Emitted when the payer disputes the milestone and the funds go on hold.
#[contractevent(topics = ["milestone_escrow", "disputed"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneEscrowDisputed {
    #[topic]
    pub escrow_id: u32,
    #[topic]
    pub payer: Address,
    pub amount: i128,
    /// First ledger in which `cancel_milestone_escrow` will succeed
    pub reclaim_after: u32,
}

/// Emitted when the payer reclaims disputed funds after the timeout.
#[contractevent(topics = ["milestone_escrow", "cancelled"])]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneEscrowCancelled {
    #[topic]
    pub escrow_id: u32,
    #[topic]
    pub payer: Address,
    pub amount: i128,
}

// ─── Milestone escrow ────────────────────────────────────────────────────

pub(crate) fn create_milestone_escrow(
    env: Env,
    token: Address,
    payer: Address,
    recipient: Address,
    amount: i128,
    approver: Address,
    dispute_timeout: u32,
) -> u32 {
    bump_instance(&env);
    payer.require_auth();

    if amount <= 0 {
        panic!("Escrow amount must be positive");
    }
    if dispute_timeout == 0 || dispute_timeout > MAX_DISPUTE_TIMEOUT {
        panic!("Escrow dispute timeout is out of range");
    }

    // Hold the funds in the contract until the milestone resolves.
    let contract = env.current_contract_address();
    token::Client::new(&env, &token).transfer(&payer, &contract, &amount);

    let id: u32 = env
        .storage()
        .instance()
        .get(&DataKey::MilestoneEscrowCount)
        .unwrap_or(0);

    let escrow = MilestoneEscrowRecord {
        token: token.clone(),
        payer: payer.clone(),
        recipient: recipient.clone(),
        approver: approver.clone(),
        amount,
        status: EscrowStatus::Pending,
        created_ledger: env.ledger().sequence(),
        dispute_ledger: 0,
        dispute_timeout,
    };
    save_escrow(&env, id, &escrow);
    env.storage()
        .instance()
        .set(&DataKey::MilestoneEscrowCount, &(id + 1));

    MilestoneEscrowCreated {
        escrow_id: id,
        payer,
        recipient,
        approver,
        token,
        amount,
        dispute_timeout,
    }
    .publish(&env);

    id
}

pub(crate) fn approve_milestone(env: Env, escrow_id: u32, approver: Address) {
    bump_instance(&env);
    let escrow = load_escrow(&env, escrow_id);

    approver.require_auth();
    if approver != escrow.approver {
        panic!("Only the designated approver can approve this escrow");
    }
    if escrow.status != EscrowStatus::Pending {
        panic!("Escrow is no longer pending");
    }

    let contract = env.current_contract_address();
    token::Client::new(&env, &escrow.token).transfer(&contract, &escrow.recipient, &escrow.amount);

    let mut released = escrow.clone();
    released.status = EscrowStatus::Approved;
    save_escrow(&env, escrow_id, &released);

    MilestoneEscrowApproved {
        escrow_id,
        approver,
        recipient: escrow.recipient.clone(),
        amount: escrow.amount,
    }
    .publish(&env);
}

pub(crate) fn dispute_milestone(env: Env, escrow_id: u32, payer: Address) {
    bump_instance(&env);
    let escrow = load_escrow(&env, escrow_id);

    payer.require_auth();
    if payer != escrow.payer {
        panic!("Only the payer can dispute this escrow");
    }
    if escrow.status != EscrowStatus::Pending {
        panic!("Escrow is no longer pending");
    }

    let mut disputed = escrow.clone();
    disputed.status = EscrowStatus::Disputed;
    disputed.dispute_ledger = env.ledger().sequence();
    save_escrow(&env, escrow_id, &disputed);

    MilestoneEscrowDisputed {
        escrow_id,
        payer,
        amount: escrow.amount,
        reclaim_after: reclaim_ledger(&disputed),
    }
    .publish(&env);
}

pub(crate) fn cancel_milestone_escrow(env: Env, escrow_id: u32, payer: Address) {
    bump_instance(&env);
    let escrow = load_escrow(&env, escrow_id);

    payer.require_auth();
    if payer != escrow.payer {
        panic!("Only the payer can cancel this escrow");
    }
    if escrow.status != EscrowStatus::Disputed {
        panic!("Escrow is not disputed");
    }
    if env.ledger().sequence() < reclaim_ledger(&escrow) {
        panic!("Escrow dispute timeout has not elapsed");
    }

    let contract = env.current_contract_address();
    token::Client::new(&env, &escrow.token).transfer(&contract, &escrow.payer, &escrow.amount);

    let mut cancelled = escrow.clone();
    cancelled.status = EscrowStatus::Cancelled;
    save_escrow(&env, escrow_id, &cancelled);

    MilestoneEscrowCancelled {
        escrow_id,
        payer,
        amount: escrow.amount,
    }
    .publish(&env);
}

pub(crate) fn get_milestone_escrow(env: Env, escrow_id: u32) -> MilestoneEscrowRecord {
    bump_instance(&env);
    load_escrow(&env, escrow_id)
}

pub(crate) fn get_milestone_escrow_count(env: Env) -> u32 {
    bump_instance(&env);
    env.storage()
        .instance()
        .get(&DataKey::MilestoneEscrowCount)
        .unwrap_or(0)
}

fn reclaim_ledger(escrow: &MilestoneEscrowRecord) -> u32 {
    escrow.dispute_ledger.saturating_add(escrow.dispute_timeout)
}

fn load_escrow(env: &Env, escrow_id: u32) -> MilestoneEscrowRecord {
    let key = DataKey::MilestoneEscrow(escrow_id);
    let escrow: MilestoneEscrowRecord = env
        .storage()
        .persistent()
        .get(&key)
        .expect("Escrow not found");
    env.storage()
        .persistent()
        .extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
    escrow
}

fn save_escrow(env: &Env, escrow_id: u32, escrow: &MilestoneEscrowRecord) {
    let key = DataKey::MilestoneEscrow(escrow_id);
    env.storage().persistent().set(&key, escrow);
    env.storage()
        .persistent()
        .extend_ttl(&key, ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
    env.storage()
        .instance()
        .extend_ttl(ESCROW_TTL_THRESHOLD, ESCROW_TTL_BUMP);
}

// ─── Time-locked escrow (ROADMAP v2.1) ─────────────────────────────────

pub(crate) fn open_escrow(
    env: Env,
    payer: Address,
    recipient: Address,
    amount: i128,
    release_ledger: u32,
) -> u32 {
    bump_instance(&env);
    payer.require_auth();

    if amount <= 0 {
        panic!("Escrow amount must be positive");
    }
    if release_ledger <= env.ledger().sequence() {
        panic!("Release ledger must be in the future");
    }

    let count: u32 = env
        .storage()
        .instance()
        .get(&DataKey::EscrowCount)
        .unwrap_or(0);

    let record = EscrowRecord {
        payer: payer.clone(),
        recipient: recipient.clone(),
        amount,
        release_ledger,
        released: false,
        cancelled: false,
    };

    env.storage()
        .instance()
        .set(&DataKey::Escrow(count), &record);
    env.storage()
        .instance()
        .set(&DataKey::EscrowCount, &(count + 1));

    env.events().publish(
        (
            Symbol::new(&env, "MicroPayContract"),
            Symbol::new(&env, "open_escrow"),
            payer,
            recipient,
        ),
        count,
    );

    count
}

pub(crate) fn release_escrow(env: Env, escrow_id: u32) -> i128 {
    bump_instance(&env);
    let mut record: EscrowRecord = env
        .storage()
        .instance()
        .get(&DataKey::Escrow(escrow_id))
        .expect("Escrow not found");

    if record.released {
        panic!("Escrow already released");
    }
    if record.cancelled {
        panic!("Escrow was cancelled");
    }
    if env.ledger().sequence() < record.release_ledger {
        panic!("Escrow is still locked");
    }

    record.released = true;
    env.storage()
        .instance()
        .set(&DataKey::Escrow(escrow_id), &record);

    env.events().publish(
        (
            Symbol::new(&env, "MicroPayContract"),
            Symbol::new(&env, "release_escrow"),
            record.recipient.clone(),
            escrow_id,
        ),
        record.amount,
    );

    record.amount
}

pub(crate) fn cancel_escrow(env: Env, escrow_id: u32, payer: Address) -> i128 {
    bump_instance(&env);
    payer.require_auth();

    let mut record: EscrowRecord = env
        .storage()
        .instance()
        .get(&DataKey::Escrow(escrow_id))
        .expect("Escrow not found");

    if record.payer != payer {
        panic!("Not escrow payer");
    }
    if record.released {
        panic!("Escrow already released");
    }
    if record.cancelled {
        panic!("Escrow already cancelled");
    }
    if env.ledger().sequence() >= record.release_ledger {
        panic!("Escrow lock has expired");
    }

    record.cancelled = true;
    env.storage()
        .instance()
        .set(&DataKey::Escrow(escrow_id), &record);

    env.events().publish(
        (
            Symbol::new(&env, "MicroPayContract"),
            Symbol::new(&env, "cancel_escrow"),
            payer,
            escrow_id,
        ),
        record.amount,
    );

    record.amount
}

pub(crate) fn get_escrow(env: Env, escrow_id: u32) -> EscrowRecord {
    bump_instance(&env);
    env.storage()
        .instance()
        .get(&DataKey::Escrow(escrow_id))
        .expect("Escrow not found")
}
