[![Internet Computer portal](https://img.shields.io/badge/InternetComputer-grey?logo=internet%20computer&style=for-the-badge)](https://internetcomputer.org)
[![GitHub license](https://img.shields.io/badge/license-Apache%202.0-blue.svg?logo=apache&style=for-the-badge)](LICENSE)

# ckSOL

> [!IMPORTANT]
> The ckSOL minter is under active development and subject to change. Access to the repository has been opened to allow for early feedback. Check back regularly for updates.

<img src="static/images/cksol-token.svg" alt="ckSOL logo" width="100" />

ckSOL is a [chain-key token](https://internetcomputer.org/how-it-works/chain-key-tokens/) on the [Internet Computer](https://internetcomputer.org/) that is backed 1:1 by SOL, the native token of the [Solana](https://solana.com/) blockchain.

Each ckSOL is backed by exactly 1 SOL held by the ckSOL minter canister. ckSOL can be converted to SOL at any time, and vice versa.

## Table of Contents

- [How It Works](#how-it-works)
  - [Deposit: SOL → ckSOL](#deposit-sol--cksol)
  - [Withdrawal: ckSOL → SOL](#withdrawal-cksol--sol)
- [Architecture](#architecture)
- [Interacting via the CLI](#interacting-via-the-cli)
- [Repository Structure](#repository-structure)
- [Development](#development)
  - [Prerequisites](#prerequisites)
  - [Building](#building)
  - [Testing](#testing)
- [Related Projects](#related-projects)
- [Contributing](#contributing)
- [License](#license)

## How It Works

The ckSOL minter canister is the core component of the system. It manages the conversion between SOL and ckSOL, maintains custody of the SOL backing all outstanding ckSOL tokens, and interacts with the Solana blockchain via the [SOL RPC canister](https://github.com/dfinity/sol-rpc-canister).

The ckSOL token itself is implemented as an [ICRC-1/ICRC-2](https://github.com/dfinity/ICRC-1) ledger canister.

The minter controls one or more Solana addresses derived from a [threshold Schnorr over Ed25519](https://internetcomputer.org/docs/references/ic-interface-spec#ic-sign-with-schnorr) public key and a per-account derivation path. No private key ever exists in plaintext — Solana transactions are signed via the IC management canister's `sign_with_schnorr` API (`SchnorrAlgorithm::Ed25519`).

### Deposit: SOL → ckSOL

1. **Get a deposit address.** Call `get_deposit_address` on the minter with your ICP principal (and an optional subaccount). The minter returns a Solana address derived specifically for your account.

2. **Send SOL.** Transfer SOL to that deposit address from any Solana wallet.

3. **Notify the minter.** Call `process_deposit` on the minter with the Solana transaction signature and the same owner/subaccount used in step 1. This call requires attaching cycles (see `process_deposit_required_cycles` in `get_minter_info`). The minter:
   - Fetches the transaction from Solana via the SOL RPC canister.
   - Verifies it is a valid transfer to your deposit address.
   - Mints the corresponding amount of ckSOL (minus the deposit fee) to your ICRC-1 ledger account.

4. **Consolidation.** The minter periodically consolidates funds from individual deposit addresses into its main Solana account.

```mermaid
sequenceDiagram
    actor User
    participant Minter as ckSOL Minter
    participant Ledger as ckSOL Ledger
    participant Solana

    User->>Minter: get_deposit_address(owner, subaccount)
    Minter-->>User: deposit_address

    User->>Solana: transfer SOL to deposit_address
    Solana-->>User: tx_signature

    User->>Minter: process_deposit(owner, subaccount, signature)
    Minter->>Solana: fetch & verify transaction
    Minter->>Ledger: mint with icrc1_transfer(to=user, amount - deposit_fee)
    Ledger-->>Minter: block_index
    Minter-->>User: Minted { block_index, minted_amount }
```

### Withdrawal: ckSOL → SOL

1. **Approve the minter.** Grant the minter an [ICRC-2](https://github.com/dfinity/ICRC-1/blob/main/standards/ICRC-2/README.md) allowance on your ckSOL ledger account.

2. **Submit a withdrawal request.** Call `withdraw` on the minter with the destination Solana address and the amount in [lamports](https://solana.com/docs/terminology#lamport). The minter:
   - Burns the requested ckSOL from your ledger account via [icrc2_transfer_from](https://github.com/dfinity/ICRC-1/blob/main/standards/ICRC-2/README.md#icrc2_transfer_from).
   - Queues the corresponding SOL transfer.

3. **Transaction submission.** The minter constructs a Solana transaction, signs it using chain-key Ed25519, and submits it via the SOL RPC canister.

4. **Monitor status.** Call `withdrawal_status` with the ledger burn index returned by `withdraw` to track the status of your withdrawal request (`Pending` → `TxSent` → `TxFinalized`).

```mermaid
sequenceDiagram
    actor User
    participant Minter as ckSOL Minter
    participant Ledger as ckSOL Ledger
    participant Solana

    User->>Ledger: icrc2_approve(spender=minter, amount)
    Ledger-->>User: ok

    User->>Minter: withdraw(destination_address, amount)
    Minter->>Ledger: burn with icrc2_transfer_from(from=user, to=burn, amount)
    Ledger-->>Minter: burn_block_index
    Minter-->>User: burn_block_index

    Note over Minter,Solana: (processed asynchronously by the minter)
    Minter->>Solana: submit SOL transfer to destination_address

    User->>Minter: withdrawal_status(burn_block_index)
    Minter-->>User: TxFinalized(Success)
```

## Architecture

```mermaid
graph TD
    subgraph IC["Internet Computer"]
        Minter["ckSOL Minter\n(this repo)"]
        Ledger["ckSOL Ledger"]
        RPC["SOL RPC Canister"]
        Minter --> Ledger
        Minter --> RPC
    end
    RPC -->|HTTPS outcalls| Providers["Solana JSON-RPC providers\n(Ankr, Helius, dRPC, ...)"]
    Providers --> Solana["Solana Blockchain"]
```

**ckSOL Minter** — The main canister in this repository. It manages the deposit and withdrawal lifecycle, holds custody of SOL via chain-key addresses, signs Solana transactions using threshold Schnorr over Ed25519, and interacts with the ckSOL ledger.

**ckSOL Ledger** — A standard [ICRC-1/ICRC-2](https://github.com/dfinity/ICRC-1) ledger canister. It tracks ckSOL balances and processes mints and burns as instructed by the minter.

**SOL RPC Canister** — A shared infrastructure canister on the Internet Computer that relays Solana JSON-RPC calls to multiple providers via HTTPS outcalls and aggregates their responses. See the [SOL RPC canister repository](https://github.com/dfinity/sol-rpc-canister) for details.

## Interacting via the CLI

You can interact with the ckSOL minter using [`icp-cli`](https://github.com/dfinity/icp-cli). Pass `-e prod` (or `-e staging`) to target the corresponding environment defined in `icp.yaml`.

### Get minter info

Query fees, minimum amounts, and the current minter balance:

```sh
icp canister call -e prod cksol_minter get_minter_info '()'
```

### Get your deposit address

Returns the Solana address you should send SOL to in order to deposit. When `owner` is `null`, it defaults to your calling identity's principal:

```sh
icp canister call -e prod cksol_minter get_deposit_address \
  '(record { owner = null; subaccount = null })'
```

### Notify the minter of a deposit

After sending SOL to your deposit address, call `process_deposit` with the Solana transaction signature to trigger minting. Pass the same `owner`/`subaccount` used when calling `get_deposit_address` — when `owner` is `null`, it defaults to your calling identity's principal. Replace `<SIGNATURE>` with the base-58 encoded transaction signature.

> **Note:** This call requires attaching cycles — check the required amount via `get_minter_info` (`process_deposit_required_cycles` field). If your identity does not hold cycles directly, you can [convert ICP to cycles](https://cli.internetcomputer.org/0.2/guides/tokens-and-cycles/#converting-icp-to-cycles) first, or route the call through a proxy canister using `--proxy <proxy-principal> --cycles <amount>`.

```sh
icp canister call -e prod cksol_minter process_deposit \
  '(record { owner = null; subaccount = null; signature = "<SIGNATURE>" })'
```

A successful response looks like:

```
(variant { Ok = variant { Minted = record { block_index = 42; minted_amount = 990_000_000; deposit_id = ... } } })
```

### Submit a withdrawal request

Burns ckSOL from your ledger account and initiates a transfer of the equivalent SOL to the given Solana address. Replace `<SOLANA_ADDRESS>` with the destination address and `<AMOUNT>` with the amount in lamports. The optional `from_subaccount` field defaults to `null` (the default subaccount):

```sh
icp canister call -e prod cksol_minter withdraw \
  '(record { address = "<SOLANA_ADDRESS>"; amount = <AMOUNT>; from_subaccount = null })'
```

A successful response returns the burn block index, which you can use to track the withdrawal:

```
(variant { Ok = record { block_index = 42 } })
```

### Check a withdrawal status

After calling `withdraw`, track the status using the `block_index` returned in the response:

```sh
icp canister call -e prod cksol_minter withdrawal_status '(42)'
```

## Repository Structure

```
.
├── minter/                  # ckSOL minter canister
│   ├── src/
│   │   ├── address/         # Deposit address derivation
│   │   ├── consolidate/     # Deposit consolidation logic
│   │   ├── dashboard/       # HTTP dashboard
│   │   ├── lifecycle.rs     # Canister init/upgrade and event log
│   │   ├── metrics.rs       # Prometheus metrics
│   │   ├── monitor/         # Transaction monitoring
│   │   ├── state/           # Minter state and event sourcing
│   │   ├── deposit/manual/  # Manual deposit processing
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
- `jq` — used by `./scripts/build` to generate Wasm metadata.
- `gzip` — used by `./scripts/build` to compress the output Wasm.

Install the Rust toolchain and `ic-wasm` by running:

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

The test suite has two parts:

**Unit tests and PocketIC integration tests** — no external dependencies:

```sh
cargo test --lib
cargo test -p cksol-int-tests --test tests
```

**Solana validator integration tests** — require a running [`solana-test-validator`](https://solana.com/docs/intro/installation) at `http://localhost:8899`:

```sh
solana-test-validator &
cargo test -p cksol-int-tests --test solana_test_validator
```

> Running `cargo test` without arguments will attempt all tests, including the Solana validator suite, and will fail if no validator is running.

## Related Projects

- [SOL RPC Canister](https://github.com/dfinity/sol-rpc-canister) — Solana JSON-RPC access from the Internet Computer.
- [ckETH](https://github.com/dfinity/ic/tree/master/rs/ethereum/cketh) — Chain-key Ethereum token, which ckSOL is modeled after.
- [ckBTC](https://github.com/dfinity/ic/tree/master/rs/bitcoin/ckbtc) — Chain-key Bitcoin token.
- [ICRC-1 Ledger](https://github.com/dfinity/ICRC-1) — The token standard used by the ckSOL ledger.

## Contributing

At this point we do not accept external contributions yet. External contributions will be accepted after the initial release.

## License

This project is licensed under the [Apache License 2.0](LICENSE).
