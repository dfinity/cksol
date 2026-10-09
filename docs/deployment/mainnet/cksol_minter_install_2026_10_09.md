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

## Wasm Verification

Verify that the hash of the gzipped WASM matches the proposed hash.

```
git fetch
git checkout bfeae694767f24decc7e45a0e3335f9f76163438
"./scripts/docker-build"
sha256sum ./wasms/cksol_minter.wasm.gz
```