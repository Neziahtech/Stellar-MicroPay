//! NFT-style on-chain payment receipts (#1490).

use crate::storage::{bump_instance, require_not_frozen, DataKey};
use soroban_sdk::{contracttype, Address, Env, Symbol};

/// On-chain receipt metadata minted as proof of payment.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ReceiptMetadata {
    /// The payer's Stellar address
    pub from: Address,
    /// The payee's Stellar address
    pub to: Address,
    /// Amount in stroops (1 XLM = 10_000_000 stroops)
    pub amount: i128,
    /// ISO-8601 timestamp of when the receipt was minted
    pub timestamp: u64,
    /// Optional payment memo
    pub memo: Symbol,
    /// Ledger number when this receipt was minted
    pub ledger: u32,
}

// ─── NFT Receipts ───────────────────────────────────────────────────────

pub(crate) fn mint_receipt(
    env: Env,
    from: Address,
    to: Address,
    amount: i128,
    memo: Symbol,
) -> u32 {
    bump_instance(&env);
    require_not_frozen(&env);

    from.require_auth();

    if amount <= 0 {
        panic!("Receipt amount must be positive");
    }

    let count: u32 = env
        .storage()
        .instance()
        .get(&DataKey::ReceiptCount(from.clone()))
        .unwrap_or(0);

    let receipt = ReceiptMetadata {
        from: from.clone(),
        to: to.clone(),
        amount,
        timestamp: env.ledger().timestamp(),
        memo,
        ledger: env.ledger().sequence(),
    };

    env.storage()
        .instance()
        .set(&DataKey::ReceiptRecord(from.clone(), count), &receipt);

    env.storage()
        .instance()
        .set(&DataKey::ReceiptCount(from.clone()), &(count + 1));

    env.events().publish(
        (
            Symbol::new(&env, "MicroPayContract"),
            Symbol::new(&env, "mint_receipt"),
            from,
            to,
        ),
        count,
    );

    count
}

pub(crate) fn get_receipt_count(env: Env, payer: Address) -> u32 {
    bump_instance(&env);
    env.storage()
        .instance()
        .get(&DataKey::ReceiptCount(payer))
        .unwrap_or(0)
}

pub(crate) fn get_receipt(env: Env, payer: Address, index: u32) -> ReceiptMetadata {
    bump_instance(&env);
    env.storage()
        .instance()
        .get(&DataKey::ReceiptRecord(payer, index))
        .expect("Receipt not found")
}
