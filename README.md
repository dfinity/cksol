[![CI](https://github.com/dfinity/cksol/actions/workflows/ci.yml/badge.svg)](https://github.com/dfinity/cksol/actions/workflows/ci.yml)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![Solana](https://img.shields.io/badge/Solana-mainnet-9945FF.svg)](https://solana.com/)
[![Internet Computer](https://img.shields.io/badge/Internet%20Computer-mainnet-blueviolet.svg)](https://internetcomputer.org/)

# ckSOL

<img src="static/images/cksol-token.svg" alt="ckSOL logo" width="100" />

ckSOL is a [chain-key token](https://internetcomputer.org/how-it-works/chain-key-tokens/) on the [Internet Computer](https://internetcomputer.org/) that is backed 1:1 by SOL, the native token of the [Solana](https://solana.com/) blockchain.

Each ckSOL is backed by exactly 1 SOL held by the ckSOL minter canister. ckSOL can be converted to SOL at any time, and vice versa.

📖 **[Full documentation →](https://dfinity.github.io/cksol/)**

- [How It Works](https://dfinity.github.io/cksol/usage/how-it-works.html): the deposit and withdrawal flows.
- [Command Line Interface](https://dfinity.github.io/cksol/usage/cli.html): interacting with the minter using `icp-cli`.
- [Design](https://dfinity.github.io/cksol/development/design.html): the full design rationale, including fees and flows.
- [Deployment](https://dfinity.github.io/cksol/development/deployment.html): installing the canisters and running a round trip.
- [Building and Testing](https://dfinity.github.io/cksol/development/building-and-testing.html): working on this repository.

## 🚀 Deployment

### <img src="static/images/cksol-token.svg" width="20" valign="middle"> ckSOL — 🚀 Production (Solana Mainnet)

| Canister     | Canister ID                                                                                                  |
|--------------|--------------------------------------------------------------------------------------------------------------|
| Minter       | [`lh22c-kyaaa-aaaar-qb5nq-cai`](https://dashboard.internetcomputer.org/canister/lh22c-kyaaa-aaaar-qb5nq-cai) |
| Ledger       | [`ls5lp-lqaaa-aaaar-qb5oa-cai`](https://dashboard.internetcomputer.org/canister/ls5lp-lqaaa-aaaar-qb5oa-cai) |
| Ledger Index | [`2ezyf-hqaaa-aaaar-qb6ga-cai`](https://dashboard.internetcomputer.org/canister/2ezyf-hqaaa-aaaar-qb6ga-cai) |

### <img src="static/images/ckdevnetsol-token.svg" width="20" valign="middle"> ckDevnetSOL — 🧪 Staging (Solana Devnet)

| Canister     | Canister ID                                                                                                  |
|--------------|--------------------------------------------------------------------------------------------------------------|
| Minter       | [`ljyxk-riaaa-aaaar-qb5mq-cai`](https://dashboard.internetcomputer.org/canister/ljyxk-riaaa-aaaar-qb5mq-cai) |
| Ledger       | [`la34w-haaaa-aaaar-qb5na-cai`](https://dashboard.internetcomputer.org/canister/la34w-haaaa-aaaar-qb5na-cai) |
| Ledger Index | [`2r6ji-gyaaa-aaaar-qb6fq-cai`](https://dashboard.internetcomputer.org/canister/2r6ji-gyaaa-aaaar-qb6fq-cai) |

## 🔗 Related Projects

- [SOL RPC Canister](https://github.com/dfinity/sol-rpc-canister) — Solana JSON-RPC access from the Internet Computer.
- [ckETH](https://github.com/dfinity/ic/tree/master/rs/ethereum/cketh) — Chain-key Ethereum token, which ckSOL is modeled after.
- [ckBTC](https://github.com/dfinity/ic/tree/master/rs/bitcoin/ckbtc) — Chain-key Bitcoin token.
- [ICRC-1 Ledger](https://github.com/dfinity/ICRC-1) — The token standard used by the ckSOL ledger.

## 🤝 Contributing

At this point we do not accept external contributions yet. External contributions will be accepted after the initial release.

## 📄 License

This project is licensed under the [Apache License 2.0](LICENSE).
