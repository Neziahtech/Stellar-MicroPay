//! Creator tipping: single tips, batched tips, and the operator fee (#1490).

use crate::storage::{bump_instance, require_not_frozen, DataKey};
use soroban_sdk::{contracttype, token, Address, Env, Symbol};

/// A single tip event recorded on-chain.
#[contracttype]
#[derive(Clone, Debug)]
pub struct TipRecord {
    /// The sender's Stellar address
    pub from: Address,
    /// The recipient's Stellar address
    pub to: Address,
    /// Amount in stroops (1 XLM = 10_000_000 stroops)
    pub amount: i128,
    /// Ledger number when this tip was sent
    pub ledger: u32,
}

/// Event payload emitted when a tip is sent, capturing the gross tip
/// amount and any operator fee that was deducted from it.
#[contracttype]
#[derive(Clone, Debug)]
pub struct TipEventData {
    pub amount: i128,
    pub fee_amount: i128,
}

/// Maximum operator fee the admin may configure, in basis points (5%).
pub(crate) const MAX_FEE_BPS: u32 = 500;

/// Basis-point denominator: 1 bps = 1 / 10_000.
pub(crate) const FEE_BPS_DENOMINATOR: i128 = 10_000;

// ─── Tipping ─────────────────────────────────────────────────────────────

pub(crate) fn send_tip(env: Env, token_address: Address, from: Address, to: Address, amount: i128) {
    bump_instance(&env);
    require_not_frozen(&env);

    // Require sender authorization
    from.require_auth();

    // Validate amount
    if amount <= 0 {
        panic!("Tip amount must be positive");
    }

    let fee_bps: u32 = env.storage().instance().get(&DataKey::FeeBps).unwrap_or(0);
    let fee_amount: i128 = (amount * fee_bps as i128) / FEE_BPS_DENOMINATOR;
    let net_amount: i128 = amount - fee_amount;

    // Transfer tokens via the Stellar token interface (SAC)
    let token = token::Client::new(&env, &token_address);

    if fee_amount > 0 {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Contract not initialized");
        token.transfer(&from, &admin, &fee_amount);
    }

    token.transfer(&from, &to, &net_amount);

    // Update on-chain tip totals for the recipient
    let current_total: i128 = env
        .storage()
        .instance()
        .get(&DataKey::TipTotal(to.clone()))
        .unwrap_or(0);

    let current_count: u32 = env
        .storage()
        .instance()
        .get(&DataKey::TipCount(to.clone()))
        .unwrap_or(0);

    env.storage()
        .instance()
        .set(&DataKey::TipTotal(to.clone()), &(current_total + amount));

    env.storage()
        .instance()
        .set(&DataKey::TipCount(to.clone()), &(current_count + 1));

    // Store the tip record so it can be queried later
    let record = TipRecord {
        from: from.clone(),
        to: to.clone(),
        amount,
        ledger: env.ledger().sequence(),
    };
    env.storage()
        .instance()
        .set(&DataKey::TipRecord(to.clone(), current_count), &record);

    // Emit an event for indexers, including the fee collected (if any)
    env.events().publish(
        (
            Symbol::new(&env, "MicroPayContract"),
            Symbol::new(&env, "send_tip"),
            from,
            to.clone(),
        ),
        TipEventData { amount, fee_amount },
    );
}

pub(crate) fn batch_tip(
    env: Env,
    token_address: Address,
    from: Address,
    tips: soroban_sdk::Vec<(Address, i128)>,
) {
    bump_instance(&env);
    from.require_auth();
    // `is_empty` rather than `len() == 0` (#1493 clippy gate; same behaviour).
    if tips.is_empty() {
        panic!("At least one tip is required");
    }

    let mut total = 0i128;
    let mut i = 0u32;
    while i < tips.len() {
        let (recipient, amount) = tips.get(i).unwrap();
        if amount <= 0 {
            panic!("Tip amount must be positive");
        }
        let mut j = 0u32;
        while j < i {
            let (previous, _) = tips.get(j).unwrap();
            if previous == recipient {
                panic!("Duplicate tip recipient");
            }
            j += 1;
        }
        total = total.checked_add(amount).expect("Tip total overflow");
        i += 1;
    }

    let token = token::Client::new(&env, &token_address);
    let contract = env.current_contract_address();
    token.transfer(&from, &contract, &total);

    let mut index = 0u32;
    while index < tips.len() {
        let (recipient, amount) = tips.get(index).unwrap();
        token.transfer(&contract, &recipient, &amount);

        let current_total: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TipTotal(recipient.clone()))
            .unwrap_or(0);
        let current_count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::TipCount(recipient.clone()))
            .unwrap_or(0);
        env.storage().instance().set(
            &DataKey::TipTotal(recipient.clone()),
            &(current_total + amount),
        );
        env.storage()
            .instance()
            .set(&DataKey::TipCount(recipient.clone()), &(current_count + 1));
        env.storage().instance().set(
            &DataKey::TipRecord(recipient.clone(), current_count),
            &TipRecord {
                from: from.clone(),
                to: recipient.clone(),
                amount,
                ledger: env.ledger().sequence(),
            },
        );
        env.events()
            .publish((Symbol::new(&env, "tip"), from.clone(), recipient), amount);
        index += 1;
    }
}

// ─── Fees ────────────────────────────────────────────────────────────────

pub(crate) fn set_fee_bps(env: Env, admin: Address, fee_bps: u32) {
    bump_instance(&env);
    admin.require_auth();

    let stored_admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("Contract not initialized");

    if admin != stored_admin {
        panic!("Only the admin can set the fee");
    }

    if fee_bps > MAX_FEE_BPS {
        panic!("Fee exceeds maximum allowed (500 bps)");
    }

    env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
}

pub(crate) fn get_fee_bps(env: Env) -> u32 {
    bump_instance(&env);
    env.storage().instance().get(&DataKey::FeeBps).unwrap_or(0)
}

// ─── Getters ─────────────────────────────────────────────────────────────

pub(crate) fn get_tip_total(env: Env, recipient: Address) -> i128 {
    bump_instance(&env);
    env.storage()
        .instance()
        .get(&DataKey::TipTotal(recipient))
        .unwrap_or(0)
}

pub(crate) fn get_tip_count(env: Env, recipient: Address) -> u32 {
    bump_instance(&env);
    env.storage()
        .instance()
        .get(&DataKey::TipCount(recipient))
        .unwrap_or(0)
}

pub(crate) fn get_tip_record(env: Env, recipient: Address, index: u32) -> TipRecord {
    bump_instance(&env);
    env.storage()
        .instance()
        .get(&DataKey::TipRecord(recipient, index))
        .expect("Tip record not found")
}
