//! Batch micro-payments (#1490) — currently a placeholder for ROADMAP v2.0.

use crate::storage::{bump_instance, require_not_frozen};
use soroban_sdk::{Address, Env};

pub(crate) fn batch_send(
    env: Env,
    _from: Address,
    _recipients: soroban_sdk::Vec<Address>,
    _amounts: soroban_sdk::Vec<i128>,
) {
    bump_instance(&env);
    require_not_frozen(&env);
    panic!("Batch payments coming in v2.0 — see ROADMAP.md");
}
