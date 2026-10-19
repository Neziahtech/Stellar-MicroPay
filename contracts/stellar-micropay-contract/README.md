# Stellar MicroPay — Soroban Contract

This directory contains the Soroban smart contract for Stellar MicroPay.

## Overview

The contract is written in Rust and compiled to WebAssembly (WASM) for deployment on the Stellar network via Soroban.

**Current features (v0.1):**
- Contract initialization with admin
- On-chain tip recording with event emission
- Tip total and count queries per recipient
- Optional operator fee (basis points) collected on every tip
- Streaming payments (open/claim/top-up/close, with pause/resume)
- Time-locked escrow (open/release/cancel)
- Milestone escrow: funds held by the contract, released by a designated
  approver, or reclaimed by the payer once a dispute times out
- Placeholder stub for batch payments

## Source layout (#1490)

The contract code lives in feature modules instead of one `lib.rs`. `src/lib.rs`
keeps the public surface: the `#[contract]` type, the error enum, and a single
`#[contractimpl]` block whose entry points are one-line delegations into the
modules. Signatures, parameter names, and doc comments (which the Soroban spec
embeds) are unchanged, so the compiled spec is identical to the pre-split one.

| File | Responsibility |
| --- | --- |
| `src/lib.rs` | Contract entry points (delegations), `ContractError`, shared re-exports |
| `src/storage.rs` | `DataKey` storage keys, TTL constants, `bump_instance`, `require_not_frozen` |
| `src/admin.rs` | Initialization, upgrade, admin rotation, freeze/unfreeze |
| `src/tips.rs` | `send_tip`, `batch_tip`, operator fee, tip records and totals |
| `src/receipts.rs` | NFT payment receipts (mint + readers) |
| `src/streams.rs` | Streaming payments: open/claim/top-up/close/pause/resume |
| `src/escrow.rs` | Plain escrow and milestone escrow lifecycles, milestone events |
| `src/batch.rs` | `batch_send` placeholder |
| `src/tests/` | One test module per feature plus shared fixtures in `tests/mod.rs`; randomized escrow invariants in `tests/escrow_properties.rs` and `tests/stream_properties.rs` |

## Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add WASM target
rustup target add wasm32v1-none

# Install Stellar CLI
cargo install --locked stellar-cli
```

## Build

```bash
stellar contract build
```

Output: `target/wasm32v1-none/release/stellar_micropay_contract.wasm`
(relative to the workspace root, not this directory)

## Test

```bash
cargo test
```

The Soroban SDK build script wants the spec-shaking marker for host-target
compiles (`cargo test`, `cargo check`, `cargo clippy`), the same one CI sets:

```bash
SOROBAN_SDK_BUILD_SYSTEM_SUPPORTS_SPEC_SHAKING_V2=1 cargo test
```

## Lint and format (#1493)

CI gates every contract change on these two commands; run them locally before
pushing (Lefthook also runs them on commit when Rust files are staged):

```bash
cargo fmt --all -- --check
SOROBAN_SDK_BUILD_SYSTEM_SUPPORTS_SPEC_SHAKING_V2=1 \
  cargo clippy --workspace --all-targets -- --deny warnings
```

Clippy findings are fixed in the code rather than blanket-suppressed; any
`#[allow(...)]` carries a comment explaining why.

## Security review (#1121)

Every entry point in `src/lib.rs` was reviewed for authorization correctness,
and the outcome is documented as a doc comment above each function.

| Entry point | Authorization | Review note |
| --- | --- | --- |
| `initialize` | `admin` | Was missing — see finding 1 |
| `send_tip` | `from` | Auth covers `(token_address, from, to, amount)`; amount validated first |
| `mint_receipt` | `from` | Auth covers `(from, to, amount, memo)`; records keyed by the authenticated payer |
| `get_tip_total`, `get_tip_count`, `get_admin`, `get_tip_record`, `get_receipt_count`, `get_receipt` | none (read-only) | No state is written and the values are public |
| `create_escrow`, `batch_send` | none (stubs) | Always panic before touching state |

### Findings fixed

1. **`initialize` stored an admin without authorization (critical).** It wrote
   `DataKey::Admin` without any auth check, so any account could install an
   arbitrary admin. Fixed by calling `admin.require_auth()` before the write.
2. **`require_auth` ran before argument validation (low)** in `send_tip` and
   `mint_receipt`. A panicking sub-call does not roll back sibling effects in
   the same transaction when the contract is invoked by another contract, so a
   rejected call could leave a satisfied auth entry behind for the payer.
   Fixed by validating `amount` before requesting authorization.

### Residual risk (documented, not changed)

- `initialize` is a separate invocation from the deploy, so the first caller
  can still install *themselves* as admin: `require_auth` proves the stored
  admin consented to the role, it cannot prove that address is the deployer.
  Deploy through a `__constructor` (deploy-time initialization) or invoke
  `initialize` in the same transaction as the deploy to remove that window.
- Confirmed already correct: the nested SAC `transfer` in `send_tip` sits under
  the sender's authorization; auth arguments match invocation arguments, so
  argument swapping is rejected by the host; `mint_receipt` writes are keyed by
  the authenticated payer; the getters are read-only; the escrow and batch
  stubs always panic.

### Auth test coverage

`cargo test` exercises every `require_auth` call site, including the negative
cases that must fail:

- `test_initialize_requires_admin_auth` — the admin is the sole authorizer.
- `test_initialize_rejects_unauthorized_admin` — no authorization at all fails,
  and the contract stays uninitialized.
- `test_initialize_rejects_authorization_of_another_address` — authorizing a
  different address cannot install another account as admin.
- `test_send_tip_moves_funds_and_requires_from_auth` / `test_send_tip_rejects_unauthorized_caller`
  — funds move under the sender's authorization, and an unauthorized caller
  moves nothing.
- `test_mint_receipt_requires_from_auth` / `test_mint_receipt_rejects_unauthorized_caller`
  — receipts are minted only under the payer's authorization.
- `test_failed_mint_does_not_consume_auth` — a rejected call consumes no
  authorization and leaves no state behind.

## Deploy to Testnet

```bash
# Configure your identity
stellar keys generate --global alice --network testnet

# Fund with Friendbot
stellar keys fund alice --network testnet

# Deploy
stellar contract deploy \
  --wasm target/wasm32v1-none/release/stellar_micropay_contract.wasm \
  --source alice \
  --network testnet
```

## Local network

You can deploy and exercise the contract without testnet by running a
standalone Stellar network in Docker. The `stellar` service in the root
`docker-compose.yml` runs `stellar/quickstart` in `--local` mode with Horizon,
Soroban RPC and Friendbot on port 8000. It sits behind the `localnet` profile,
so `docker compose up` on its own does not start it.

Requirements: Docker, the Stellar CLI and the Rust toolchain from
[Prerequisites](#prerequisites).

From the repository root:

```bash
make localnet-up      # start quickstart and wait until Soroban RPC reports healthy
make deploy-local     # register the "local" CLI network, fund local-admin, build, deploy, initialize
make smoke-local      # read-only checks: get_admin, get_fee_bps, is_frozen
make localnet-down    # stop the container
```

`make deploy-local` writes the contract ID to `.contract-id.local` (git-ignored)
and always deploys a fresh instance, because a standalone network loses its
state when the container is recreated. The deployer identity defaults to
`local-admin`; override it with `make deploy-local LOCAL_IDENTITY=me`.

Under the hood these targets run:

```bash
stellar network add local \
  --rpc-url http://localhost:8000/rpc \
  --network-passphrase "Standalone Network ; February 2017"
stellar keys generate local-admin --network local --fund
STELLAR_IDENTITY=local-admin ./scripts/deploy-contract.sh local
```

### Pointing the app at the local network

| Component | Variable | Local value |
| --- | --- | --- |
| Backend | `STELLAR_NETWORK` | `testnet` (keeps the Horizon/SDK defaults; see note below) |
| Backend | `HORIZON_URL` | `http://localhost:8000` (`http://stellar:8000` from inside compose) |
| Backend | `SOROBAN_RPC_URL` | `http://localhost:8000/rpc` (`http://stellar:8000/rpc` from inside compose) |
| Backend | `CONTRACT_ID` | contents of `.contract-id.local` |
| Frontend | `NEXT_PUBLIC_HORIZON_URL` | `http://localhost:8000` |
| Frontend | `NEXT_PUBLIC_CONTRACT_ID` | contents of `.contract-id.local` |

In the frontend you can also pick **Custom** in Settings and enter
`http://localhost:8000` as the Horizon URL.

Limitation: the frontend and the backend Turrets/SEP-10 code currently sign
with either the testnet or the public passphrase, never the standalone one.
Reading contract state and Horizon data works against the local network, but
wallet-signed transactions from the UI will be rejected until the app learns a
configurable passphrase. Use the Stellar CLI (as above) for write calls.

## Invoke

```bash
# Initialize
stellar contract invoke \
  --id <CONTRACT_ID> \
  --source alice \
  --network testnet \
  -- initialize \
  --admin <YOUR_PUBLIC_KEY>

# Send a tip
stellar contract invoke \
  --id <CONTRACT_ID> \
  --source alice \
  --network testnet \
  -- send_tip \
  --token_address <XLM_SAC_ADDRESS> \
  --from <SENDER_ADDRESS> \
  --to <RECIPIENT_ADDRESS> \
  --amount 1000000

# Check tip total
stellar contract invoke \
  --id <CONTRACT_ID> \
  --network testnet \
  -- get_tip_total \
  --recipient <RECIPIENT_ADDRESS>

# Set the operator fee (in basis points, e.g. 50 = 0.5%, capped at 500 = 5%)
stellar contract invoke \
  --id <CONTRACT_ID> \
  --source alice \
  --network testnet \
  -- set_fee_bps \
  --admin <YOUR_PUBLIC_KEY> \
  --fee_bps 50
```

## Milestone escrow

Funds are held by the contract until a third-party `approver` confirms the
milestone. The payer can freeze the escrow by disputing it, and gets the funds
back once `dispute_timeout` ledgers have elapsed.

```bash
# Lock funds: payer -> contract, released to recipient by approver
stellar contract invoke \
  --id <CONTRACT_ID> \
  --source alice \
  --network testnet \
  -- create_milestone_escrow \
  --token <XLM_SAC_ADDRESS> \
  --payer <PAYER_ADDRESS> \
  --recipient <RECIPIENT_ADDRESS> \
  --amount 5000000 \
  --approver <APPROVER_ADDRESS> \
  --dispute_timeout 500

# Release the funds to the recipient (approver only, escrow must be pending)
stellar contract invoke \
  --id <CONTRACT_ID> \
  --source carol \
  --network testnet \
  -- approve_milestone \
  --escrow_id 0 \
  --approver <APPROVER_ADDRESS>

# Freeze the funds pending resolution (payer only)
stellar contract invoke \
  --id <CONTRACT_ID> \
  --source alice \
  --network testnet \
  -- dispute_milestone \
  --escrow_id 0 \
  --payer <PAYER_ADDRESS>

# Reclaim a disputed escrow after the timeout has elapsed (payer only)
stellar contract invoke \
  --id <CONTRACT_ID> \
  --source alice \
  --network testnet \
  -- cancel_milestone_escrow \
  --escrow_id 0 \
  --payer <PAYER_ADDRESS>

# Read the escrow record (status: pending / approved / disputed / cancelled)
stellar contract invoke \
  --id <CONTRACT_ID> \
  --network testnet \
  -- get_milestone_escrow \
  --escrow_id 0
```

Every state change publishes a `milestone_escrow` event whose second topic is
`created`, `approved`, `disputed` or `cancelled`, followed by the escrow id and
the address that authorised the change, so indexers can follow an escrow without
reading storage.

Rules the contract enforces:

| Action | Who | Preconditions |
| --- | --- | --- |
| `create_milestone_escrow` | payer | `amount > 0`, `0 < dispute_timeout <= 50000` ledgers |
| `approve_milestone` | the escrow's `approver` | status `pending` |
| `dispute_milestone` | the escrow's `payer` | status `pending` |
| `cancel_milestone_escrow` | the escrow's `payer` | status `disputed` and `dispute_timeout` ledgers elapsed since the dispute |

A disputed escrow can no longer be approved: once the payer disputes, the only
ways out are the payer reclaiming the funds after the timeout, or waiting it out
and creating a new escrow.

## Troubleshooting (#153)

Soroban SDK 28 requires the Stellar CLI to set build metadata and the
`wasm32v1-none` target on current Rust toolchains. Build with
`stellar contract build` as shown above; a direct `cargo build` or the old
`wasm32-unknown-unknown` target may fail even when the contract source is valid.

## Error Reference

The contract does not define a `#[contracterror]` enum — every failure is a
plain `panic!` or `.expect()` call, so there is no numeric error code to
match on client-side. Instead, catch the failed invocation and match on the
panic message (available in the transaction's diagnostic events when
simulated or submitted with debug info enabled).

| Error message                                       | Trigger condition                                                                 | Affected function(s)                          |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------- | ------------------------------------------------ |
| `Contract already initialized`                       | `initialize` is called a second time (the `Admin` storage key is already set).      | `initialize`                                      |
| `Contract not initialized`                            | A function that requires the admin address reads it before `initialize` was called. | `get_admin`, `send_tip`, `set_fee_bps`            |
| `Tip amount must be positive`                         | `send_tip` is called with `amount <= 0`.                                            | `send_tip`                                        |
| `Tip record not found`                                | `get_tip_record` is called with a `(recipient, index)` pair that was never stored.  | `get_tip_record`                                  |
| `Receipt amount must be positive`                     | `mint_receipt` is called with `amount <= 0`.                                        | `mint_receipt`                                    |
| `Receipt not found`                                   | `get_receipt` is called with a `(payer, index)` pair that was never stored.         | `get_receipt`                                     |
| `Only the admin can set the fee`                      | `set_fee_bps` is called with an `admin` argument that doesn't match the stored admin, even if that address authorized the call. | `set_fee_bps`      |
| `Fee exceeds maximum allowed (500 bps)`               | `set_fee_bps` is called with `fee_bps > 500` (the 5% cap).                          | `set_fee_bps`                                     |
| `Escrow payments coming in v2.1 — see ROADMAP.md`     | `create_escrow` is called at all — it's an unimplemented placeholder.               | `create_escrow`                                   |
| `Batch payments coming in v2.0 — see ROADMAP.md`      | `batch_send` is called at all — it's an unimplemented placeholder.                  | `batch_send`                                      |

`send_tip` and `mint_receipt` also panic implicitly if the caller isn't the
address that authorized the invocation (`from.require_auth()` /
`admin.require_auth()` failing), which the Soroban host reports as its own
authorization error rather than one of the messages above.

## XLM SAC Address (Testnet)

The Stellar Asset Contract address for native XLM on testnet:
```
CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC
```

## Roadmap

- **v2.1** — Escrow payments with time-lock release
- **v2.0** — Batch micro-payment transactions
- **v1.4** — Creator tip pages

See [ROADMAP.md](../../ROADMAP.md) for full details.
