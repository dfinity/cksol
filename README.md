[![Internet Computer portal](https://img.shields.io/badge/InternetComputer-grey?logo=internet%20computer&style=for-the-badge)](https://internetcomputer.org)
[![GitHub license](https://img.shields.io/badge/license-Apache%202.0-blue.svg?logo=apache&style=for-the-badge)](LICENSE)

# ckSOL

<img src="static/images/cksol-token.svg" alt="ckSOL logo" width="100" />

ckSOL is a [chain-key token](https://internetcomputer.org/how-it-works/chain-key-tokens/) on the [Internet Computer](https://internetcomputer.org/) that is backed 1:1 by SOL, the native token of the [Solana](https://solana.com/) blockchain.

Each ckSOL is backed by exactly 1 SOL held by the ckSOL minter canister. ckSOL can be converted back to SOL at any time, and vice versa.

## Table of Contents

- [How It Works](#how-it-works)
  - [Deposit: SOL → ckSOL](#deposit-sol--cksol)
  - [Withdrawal: ckSOL → SOL](#withdrawal-cksol--sol)
- [Architecture](#architecture)
- [Repository Structure](#repository-structure)
- [Development](#development)
  - [Prerequisites](#prerequisites)
  - [Building](#building)
  - [Testing](#testing)
- [Related Projects](#related-projects)
- [License](#license)

## How It Works

The ckSOL minter canister is the core component of the system. It manages the conversion between SOL and ckSOL, maintains custody of the SOL backing all outstanding ckSOL tokens, and interacts with the Solana blockchain via the [SOL RPC canister](https://github.com/dfinity/sol-rpc-canister).

The ckSOL token itself is implemented as an [ICRC-1](https://github.com/dfinity/ICRC-1) ledger canister.

The minter controls one or more Solana addresses derived from [chain-key Ed25519 signatures](https://internetcomputer.org/docs/references/ic-interface-spec#ic-sign-with-schnorr). No private key ever exists in plaintext — transactions are signed by the Internet Computer's threshold signature protocol.

### Deposit: SOL → ckSOL

1. **Get a deposit address.** Call `get_deposit_address` on the minter with your ICP principal (and an optional subaccount). The minter returns a Solana address derived specifically for your account.

2. **Send SOL.** Transfer SOL to that deposit address from any Solana wallet.

3. **Notify the minter.** Call `update_balance` on the minter with the Solana transaction signature. The minter:
   - Fetches the transaction from Solana via the SOL RPC canister.
   - Verifies it is a valid transfer to your deposit address.
   - Mints the corresponding amount of ckSOL (minus the deposit fee) to your ICRC-1 ledger account.

4. **Consolidation.** The minter periodically consolidates funds from individual deposit addresses into its main Solana account. A small consolidation fee is charged in cycles when calling `update_balance`.

### Withdrawal: ckSOL → SOL

1. **Approve the minter.** Grant the minter an [ICRC-2](https://github.com/dfinity/ICRC-1/blob/main/standards/ICRC-2/README.md) allowance on your ckSOL ledger account.

2. **Submit a withdrawal request.** Call `withdraw` on the minter with the destination Solana address and the amount in [lamports](https://solana.com/docs/terminology#lamport). The minter:
   - Burns the requested ckSOL from your ledger account (via ICRC-2 transfer-from).
   - Queues the corresponding SOL transfer.

3. **Transaction submission.** The minter constructs a Solana transaction, signs it using chain-key Ed25519, and submits it via the SOL RPC canister.

4. **Monitor status.** Call `withdrawal_status` with the ledger burn index returned by `withdraw` to track the status of your withdrawal request (`Pending` → `TxSent` → `TxFinalized`).

## Architecture

```
                    Internet Computer
  ┌─────────────────────────────────────────────────────────────────────┐
  │                                                                     │
  │  ┌─────────────────────┐       ┌───────────────────────────────┐   │
  │  │   ckSOL Minter      │──────▶│   ckSOL Ledger (ICRC-1)       │   │
  │  │   (this repo)       │◀──────│                               │   │
  │  └──────────┬──────────┘       └───────────────────────────────┘   │
  │             │                                                       │
  │             │ HTTPS outcalls                                        │
  │             ▼                                                       │
  │  ┌─────────────────────┐                                           │
  │  │   SOL RPC Canister  │                                           │
  │  │   (tghme-zyaaa-...  │                                           │
  │  └──────────┬──────────┘                                           │
  │             │                                                       │
  └─────────────┼───────────────────────────────────────────────────────┘
                │
                ▼ JSON-RPC (HTTPS outcalls)
         ┌──────────────┐
         │    Solana    │
         │  Blockchain  │
         └──────────────┘
```

**ckSOL Minter** — The main canister in this repository. It manages the deposit and withdrawal lifecycle, holds custody of SOL via chain-key addresses, signs Solana transactions using threshold Ed25519, and interacts with the ckSOL ledger.

**ckSOL Ledger** — A standard ICRC-1/ICRC-2 ledger canister. It tracks ckSOL balances and processes mints and burns as instructed by the minter.

**SOL RPC Canister** — A shared infrastructure canister on the Internet Computer that relays Solana JSON-RPC calls to multiple providers via HTTPS outcalls and aggregates their responses. See the [SOL RPC canister repository](https://github.com/dfinity/sol-rpc-canister) for details.

## Repository Structure

```
.
├── minter/                  # ckSOL minter canister
│   ├── src/
│   │   ├── address/         # Deposit address derivation
│   │   ├── consolidate/     # Deposit consolidation logic
│   │   ├── dashboard/       # HTTP dashboard
│   │   ├── lifecycle/       # Canister init/upgrade and event log
│   │   ├── metrics/         # Prometheus metrics
│   │   ├── monitor/         # Transaction monitoring
│   │   ├── state/           # Minter state and event sourcing
│   │   ├── update_balance/  # Deposit processing
│   │   ├── withdraw/        # Withdrawal processing
│   │   └── ...
│   └── cksol_minter.did     # Candid interface
├── libs/
│   ├── types/               # Public ckSOL types (cksol-types crate)
│   └── types-internal/      # Internal types and event definitions
├── integration_tests/       # End-to-end tests using PocketIC
└── scripts/
    ├── build                # Build script for the minter Wasm
    └── bootstrap            # Install build dependencies
```

## Development

### Prerequisites

- [Rust](https://rustup.rs/) — the correct toolchain version is pinned in `rust-toolchain.toml`.
- [`ic-wasm`](https://github.com/dfinity/ic-wasm) version 0.3.5 — used for Wasm post-processing.

Install all dependencies by running:

```sh
./scripts/bootstrap
```

### Building

Build the minter Wasm:

```sh
./scripts/build --cksol_minter
```

The resulting `cksol_minter.wasm.gz` will be written to the repository root.

You can also build the Rust workspace directly (without Wasm post-processing):

```sh
cargo build
```

### Testing

Run unit and integration tests:

```sh
cargo test
```

## Related Projects

- [SOL RPC Canister](https://github.com/dfinity/sol-rpc-canister) — Solana JSON-RPC access from the Internet Computer.
- [ckETH](https://github.com/dfinity/ic/tree/master/rs/ethereum/cketh) — Chain-key Ethereum token, which ckSOL is modeled after.
- [ckBTC](https://github.com/dfinity/ic/tree/master/rs/bitcoin/ckbtc) — Chain-key Bitcoin token.
- [ICRC-1 Ledger](https://github.com/dfinity/ICRC-1) — The token standard used by the ckSOL ledger.

## License

This project is licensed under the [Apache License 2.0](LICENSE).
