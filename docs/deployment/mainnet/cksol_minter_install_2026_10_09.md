# Proposal to install the ckSOL minter canister

Repository: `https://github.com/dfinity/cksol.git`

Git hash: `bfeae694767f24decc7e45a0e3335f9f76163438`

New compressed Wasm hash: `21374814ae5ed3063762d53a0a66d9d60524792ae0d30670465172ad9c1273a8`

Install args hash: `012da32ebb0db7687a719e03a0db46aaccdc1bce9a484784f5a6ebbd39c47151`

Target canister: `lh22c-kyaaa-aaaar-qb5nq-cai`

---

## Motivation
TODO: THIS MUST BE FILLED OUT


## Install args

```
git fetch
git checkout bfeae694767f24decc7e45a0e3335f9f76163438
didc encode -d minter/cksol_minter.did -t '(MinterArg)' '(variant { Init = record {
    sol_rpc_canister_id = principal "tghme-zyaaa-aaaar-qarca-cai";
    ledger_canister_id = principal "ls5lp-lqaaa-aaaar-qb5oa-cai";
    master_key_name = variant { MainnetProdKey1 };
    solana_network = variant { Mainnet };
    deposit_sol_fee = 45_000_000_000 : nat64;
    deposit_sol_required_cycles = 1_000_000_000_000 : nat64;
    minimum_deposit_amount = 20_000_000 : nat64;
    withdrawal_fee = 1_000_000 : nat64;
    minimum_withdrawal_amount = 2_000_000 : nat64;
    nonce_accounts = vec {
      "6YJ5Tnfi6SurWYjSbUzKmb3vR2WHAw6R5jTV7B2xPk6V";
      "AkcgQ75Wfj8vywSntxRXCr965cx3innDm5YiRzgtk5JH";
      "FEtnTdiKa7riU93y8K3HancFcDHrTFnviv8ndDZ2EuDk";
      "G2VedB18ZtzqwwGX6waEGWgXi21GqQ4CLr7JHvhvDGk";
      "Au6vUGW5Db5eEPpq3G4UEN6vN5np6j5NujbNhMDo6DBm";
    };
  } })' | xxd -r -p | sha256sum
```

* [`tghme-zyaaa-aaaar-qarca-cai`](https://dashboard.internetcomputer.org/canister/tghme-zyaaa-aaaar-qarca-cai) is the SOL RPC canister, used by the minter to interact with Solana.
* [`ls5lp-lqaaa-aaaar-qb5oa-cai`](https://dashboard.internetcomputer.org/canister/ls5lp-lqaaa-aaaar-qb5oa-cai) is the governance-controlled canister ID that will become the ckSOL ledger.
* `MainnetProdKey1` is the threshold Ed25519 key used to derive the minter's Solana address [`GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax`](https://explorer.solana.com/address/GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax) and the deposit addresses, and to sign Solana transactions.
* `Mainnet` is the Solana cluster the minter operates on.
* The fees and minimum amounts below follow [Section 3.3 of the design](https://github.com/dfinity/cksol/blob/bfeae694767f24decc7e45a0e3335f9f76163438/docs/design.md#33-fees--minimum-swap-amounts). USD amounts are rough estimates with 1 SOL at about 110 USD as of October 9, 2026:
  * `deposit_sol_fee` of 45B cycles covers the threshold signature and the RPC calls of a sweep containing a single deposit.
  * `deposit_sol_required_cycles` of 1T cycles must be attached to a `deposit_sol` call. It must be at least the 10B cycles of the `getBalance` RPC call plus `deposit_sol_fee`, and unused cycles are refunded.
  * `minimum_deposit_amount` of 0.02 SOL (20,000,000 lamports, about 2.2 USD) is at least twice the rent exemption threshold plus the fee of one signature.
  * `withdrawal_fee` of 0.001 SOL (1,000,000 lamports, about 0.1 USD) covers the `getAccountInfo`, `sendTransaction` and `getTransaction` RPC calls and the threshold signature.
  * `minimum_withdrawal_amount` of 0.002 SOL (2,000,000 lamports, about 0.2 USD) is at least the withdrawal fee plus the rent exemption threshold.
* `nonce_accounts` are durable nonce accounts used to send withdrawal transactions. Each account was funded with 0.002 SOL (about 0.2 USD) and its nonce authority is the minter's address [GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax](https://explorer.solana.com/address/GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax) (can be verified offline from the canister ID alone, see the test [`should_derive_mainnet_minter_addresses_offline`](https://github.com/dfinity/cksol/blob/bfeae694767f24decc7e45a0e3335f9f76163438/minter/src/address/tests.rs#L95)):
  * [`6YJ5Tnfi6SurWYjSbUzKmb3vR2WHAw6R5jTV7B2xPk6V`](https://explorer.solana.com/address/6YJ5Tnfi6SurWYjSbUzKmb3vR2WHAw6R5jTV7B2xPk6V)
  * [`AkcgQ75Wfj8vywSntxRXCr965cx3innDm5YiRzgtk5JH`](https://explorer.solana.com/address/AkcgQ75Wfj8vywSntxRXCr965cx3innDm5YiRzgtk5JH)
  * [`FEtnTdiKa7riU93y8K3HancFcDHrTFnviv8ndDZ2EuDk`](https://explorer.solana.com/address/FEtnTdiKa7riU93y8K3HancFcDHrTFnviv8ndDZ2EuDk)
  * [`G2VedB18ZtzqwwGX6waEGWgXi21GqQ4CLr7JHvhvDGk`](https://explorer.solana.com/address/G2VedB18ZtzqwwGX6waEGWgXi21GqQ4CLr7JHvhvDGk)
  * [`Au6vUGW5Db5eEPpq3G4UEN6vN5np6j5NujbNhMDo6DBm`](https://explorer.solana.com/address/Au6vUGW5Db5eEPpq3G4UEN6vN5np6j5NujbNhMDo6DBm)

  The nonce authority of each account can be checked with `solana nonce-account <address> --url mainnet-beta`.

## Wasm Verification

Verify that the hash of the gzipped WASM matches the proposed hash.

```
git fetch
git checkout bfeae694767f24decc7e45a0e3335f9f76163438
"./scripts/docker-build"
sha256sum ./wasms/cksol_minter.wasm.gz
```