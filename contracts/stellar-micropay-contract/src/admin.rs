//! Admin controls: initialization, upgrade, admin rotation, and the
//! freeze/unfreeze circuit breaker (#1490).

use crate::storage::{bump_instance, DataKey};
use crate::ContractError;
use soroban_sdk::{Address, BytesN, Env, Symbol};

// ─── Initialization ──────────────────────────────────────────────────────

pub(crate) fn initialize(env: Env, admin: Address) {
    bump_instance(&env);
    // Ensure not already initialized
    if env.storage().instance().has(&DataKey::Admin) {
        panic!("Contract already initialized");
    }
    admin.require_auth();
    env.storage().instance().set(&DataKey::Admin, &admin);
}

// ─── Admin & Upgrade ─────────────────────────────────────────────────────

pub(crate) fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
    bump_instance(&env);
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("Contract not initialized");
    admin.require_auth();

    env.deployer().update_current_contract_wasm(new_wasm_hash);
}

pub(crate) fn set_admin(env: Env, new_admin: Address) {
    bump_instance(&env);
    let admin: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("Contract not initialized");
    admin.require_auth();

    env.storage().instance().set(&DataKey::Admin, &new_admin);
}

// ─── Emergency pause ─────────────────────────────────────────────────────

pub(crate) fn freeze(env: Env, admin: Address) {
    bump_instance(&env);
    let stored: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("Contract not initialized");
    if admin != stored {
        env.panic_with_error(ContractError::NotAdmin);
    }
    admin.require_auth();
    env.storage().instance().set(&DataKey::Frozen, &true);
    env.events()
        .publish((Symbol::new(&env, "freeze"), admin), true);
}

pub(crate) fn unfreeze(env: Env, admin: Address) {
    bump_instance(&env);
    let stored: Address = env
        .storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("Contract not initialized");
    if admin != stored {
        env.panic_with_error(ContractError::NotAdmin);
    }
    admin.require_auth();
    env.storage().instance().set(&DataKey::Frozen, &false);
    env.events()
        .publish((Symbol::new(&env, "unfreeze"), admin), false);
}

pub(crate) fn is_frozen(env: Env) -> bool {
    bump_instance(&env);
    env.storage()
        .instance()
        .get(&DataKey::Frozen)
        .unwrap_or(false)
}

// ─── Getters ─────────────────────────────────────────────────────────────

pub(crate) fn get_admin(env: Env) -> Address {
    bump_instance(&env);
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .expect("Contract not initialized")
}
