# Proposal to install the ckSOL index canister

Repository: `https://github.com/dfinity/ic.git`

Git hash: `cf41372e3d4dc1accfe2c09a7969f8bddc729dc1`

New compressed Wasm hash: `dab6808d0dfc06e5e88336d0c3d3e45e5448c6e36c2a781f3e9e09bd450f528c`

Install args hash: `0375db897c4d679bc73fdf60efe7787771604df54f8bc673750af2896fa62318`

Target canister: `2ezyf-hqaaa-aaaar-qb6ga-cai`

---

## Motivation
This proposal installs the mainnet ckSOL index to the governance-controlled canister ID [`2ezyf-hqaaa-aaaar-qb6ga-cai`](https://dashboard.internetcomputer.org/canister/2ezyf-hqaaa-aaaar-qb6ga-cai) on subnet [`pzp6e-ekpqk-3c5x7-2h6so-njoeq-mt45d-h3h6c-q3mxf-vpeq5-fk5o7-yae`](https://dashboard.internetcomputer.org/subnet/pzp6e-ekpqk-3c5x7-2h6so-njoeq-mt45d-h3h6c-q3mxf-vpeq5-fk5o7-yae).
The installed Wasm is the index of the latest ICRC ledger suite release [ledger-suite-icrc-2026-03-09](https://github.com/dfinity/ic/releases/tag/ledger-suite-icrc-2026-03-09).

ckSOL is a chain-key token on the Internet Computer backed 1:1 by SOL, the native token of the Solana blockchain.
The ckSOL index fetches the blocks of the [ckSOL ledger](https://dashboard.internetcomputer.org/canister/ls5lp-lqaaa-aaaar-qb5oa-cai) and allows clients, such as wallets, to retrieve the transactions of a given account.
See the [design document](https://github.com/dfinity/cksol/blob/bfeae694767f24decc7e45a0e3335f9f76163438/docs/design.md) for details.


## Install args

```
git fetch
git checkout cf41372e3d4dc1accfe2c09a7969f8bddc729dc1
didc encode -d rs/ledger_suite/icrc1/index-ng/index-ng.did -t '(opt IndexArg)' '(opt variant { Init = record {
    ledger_id = principal "ls5lp-lqaaa-aaaar-qb5oa-cai";
    retrieve_blocks_from_ledger_interval_seconds = null;
  } })' | xxd -r -p | sha256sum
```

* [`ls5lp-lqaaa-aaaar-qb5oa-cai`](https://dashboard.internetcomputer.org/canister/ls5lp-lqaaa-aaaar-qb5oa-cai) is the ckSOL ledger canister, whose blocks are indexed.
* `retrieve_blocks_from_ledger_interval_seconds` is unset, so the index uses its default polling interval, which adapts between 1 and 10 seconds depending on whether new blocks were found.

## Wasm Verification

Verify that the hash of the gzipped WASM matches the proposed hash.

```
git fetch
git checkout cf41372e3d4dc1accfe2c09a7969f8bddc729dc1
"./ci/container/build-ic.sh" "--canisters"
sha256sum ./artifacts/canisters/ic-icrc1-index-ng.wasm.gz
```