# Deployment

## Minter's address on Solana

Given a canister ID, the address controlled by the minter can be derived offline without installing any code.
See `should_derive_mainnet_minter_addresses_offline` for an example.

| Environment | Canister ID                                                                                                  | Solana address                                                                                                                                    |
|-------------|--------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------|
| Production  | [`lh22c-kyaaa-aaaar-qb5nq-cai`](https://dashboard.internetcomputer.org/canister/lh22c-kyaaa-aaaar-qb5nq-cai) | [`GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax`](https://explorer.solana.com/address/GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax)                |
| Staging     | [`ljyxk-riaaa-aaaar-qb5mq-cai`](https://dashboard.internetcomputer.org/canister/ljyxk-riaaa-aaaar-qb5mq-cai) | [`Br8eRkeya8hy3sHCYqtWqGeNNZ349aPSUKGVYFWKKer1`](https://explorer.solana.com/address/Br8eRkeya8hy3sHCYqtWqGeNNZ349aPSUKGVYFWKKer1?cluster=devnet) |

## Create nonce accounts

Withdrawals are sent as durable-nonce transactions, so the minter needs a pool of durable nonce accounts whose nonce authority is the minter's address.
Without them, `withdraw` returns `TemporarilyUnavailable("The durable nonce account pool is empty, no withdrawal can be processed")`.

The commands below use the [Solana CLI](https://solana.com/docs/intro/installation) and the staging minter on Devnet.
For production, use the production minter's address and `--url mainnet-beta`.

Fund a fee payer that pays for the rent of the nonce accounts:

```shell
solana-keygen new --no-bip39-passphrase -o ~/.config/solana/devnet-payer.json
solana airdrop 1 --keypair ~/.config/solana/devnet-payer.json --url devnet
```

Create the nonce accounts with the minter as nonce authority:

```shell
MINTER_ADDRESS=Br8eRkeya8hy3sHCYqtWqGeNNZ349aPSUKGVYFWKKer1
mkdir -p nonce-accounts
for i in 1 2 3; do
  solana-keygen new --no-bip39-passphrase --silent -o nonce-accounts/nonce-$i.json
  solana create-nonce-account nonce-accounts/nonce-$i.json 0.002 \
    --nonce-authority $MINTER_ADDRESS \
    --keypair ~/.config/solana/devnet-payer.json \
    --url devnet
done
```

Each account is funded with 0.002 SOL, above the rent-exempt minimum for the 80 bytes of a nonce account (see `solana rent 80`).
The `nonce-$i.json` keypairs are only needed to create the accounts: afterwards, only the nonce authority can advance the nonce or withdraw from the account.
Each in-flight withdrawal transaction occupies one nonce account until it is finalized, and a transaction batches up to 10 withdrawals.

Check that the nonce authority of each account is the minter's address:

```shell
for i in 1 2 3; do
  ADDRESS=$(solana-keygen pubkey nonce-accounts/nonce-$i.json)
  echo $ADDRESS
  solana nonce-account $ADDRESS --url devnet
done
```

The minter only parses the addresses when they are added and verifies each account when it creates a withdrawal transaction, so a wrong nonce authority only surfaces once a withdrawal is processed.

Add the accounts to the minter, either in `nonce_accounts` of the [initialization arguments](#minter) or with an upgrade:

```candid
(
  variant {
    Upgrade = record {
      nonce_accounts_to_add = opt vec { "<address-1>"; "<address-2>"; "<address-3>" };
    }
  },
)
```

The other fields of `UpgradeArgs` are optional and can be omitted.

## Minter

Initialization arguments:

<table>
<tr>
<th>Production</th>
<th>Staging</th>
</tr>
<tr>
<td>

```candid
(
  variant {
    Init = record {
      sol_rpc_canister_id = principal "tghme-zyaaa-aaaar-qarca-cai";
      ledger_canister_id = principal "ls5lp-lqaaa-aaaar-qb5oa-cai";
      master_key_name = variant { MainnetProdKey1 };
      solana_network = variant { Mainnet };
      deposit_sol_fee = 45_000_000_000 : nat64;
      deposit_sol_required_cycles = 1_000_000_000_000 : nat64;
      minimum_deposit_amount = 20_000_000 : nat64;
      withdrawal_fee = 1_000_000 : nat64;
      minimum_withdrawal_amount = 2_000_000 : nat64;
      nonce_accounts = vec {};
    }
  },
)
```

</td>
<td>

```candid
(
  variant {
    Init = record {
      sol_rpc_canister_id = principal "tghme-zyaaa-aaaar-qarca-cai";
      ledger_canister_id = principal "la34w-haaaa-aaaar-qb5na-cai";
      master_key_name = variant { MainnetProdKey1 };
      solana_network = variant { Devnet };
      deposit_sol_fee = 45_000_000_000 : nat64;
      deposit_sol_required_cycles = 1_000_000_000_000 : nat64;
      minimum_deposit_amount = 20_000_000 : nat64;
      withdrawal_fee = 1_000_000 : nat64;
      minimum_withdrawal_amount = 2_000_000 : nat64;
      nonce_accounts = vec {};
    }
  },
)
```

</td>
</tr>
</table>

The fees and minimum amounts follow [Section 3.3 of the design](design.md#33-fees--minimum-swap-amounts):

| Argument                      | Value                 | Rationale                                                                                       |
|-------------------------------|-----------------------|-------------------------------------------------------------------------------------------------|
| `deposit_sol_fee`             | 45B cycles            | Covers the threshold signature and the RPC calls of a sweep containing a single deposit.        |
| `deposit_sol_required_cycles` | 1T cycles             | Must be at least `GET_BALANCE_CYCLES` (10B) plus `deposit_sol_fee`; unused cycles are refunded. |
| `minimum_deposit_amount`      | 0.02 SOL (20,000,000) | Must be at least twice the rent exemption threshold plus the fee of one signature.              |
| `withdrawal_fee`              | 0.001 SOL (1,000,000) | Covers `getAccountInfo`, `sendTransaction`, `getSignatureStatuses` and the threshold signature. |
| `minimum_withdrawal_amount`   | 0.002 SOL (2,000,000) | Must be at least the withdrawal fee plus the rent exemption threshold.                          |
| `nonce_accounts`              | empty                 | Replace with the addresses created in [Create nonce accounts](#create-nonce-accounts).          |

## Ledger

Initialization arguments:

<table>
<tr>
<th>Production</th>
<th>Staging</th>
</tr>
<tr>
<td>

```candid
(
  variant {
    Init = record {
      minting_account = record { owner = principal "lh22c-kyaaa-aaaar-qb5nq-cai"; subaccount = null };
      fee_collector_account = opt record {
        owner = principal "lh22c-kyaaa-aaaar-qb5nq-cai";
        subaccount = opt blob "\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\0f\ee";
      };
      transfer_fee = 500 : nat;
      decimals = opt (9 : nat8);
      max_memo_length = opt (80 : nat16);
      token_symbol = "ckSOL";
      token_name = "ckSOL";
      metadata = vec {
        record { "icrc1:logo"; variant { Text = "data:image/svg+xml;base64,${LOGO}" } };
      };
      initial_balances = vec {};
      feature_flags = opt record { icrc2 = true };
      archive_options = record {
        num_blocks_to_archive = 1_000 : nat64;
        trigger_threshold = 4_200_000_000 : nat64;
        node_max_memory_size_bytes = opt (3_221_225_472 : nat64);
        max_message_size_bytes = null;
        max_transactions_per_response = null;
        cycles_for_archive_creation = opt (100_000_000_000_000 : nat64);
        controller_id = principal "r7inp-6aaaa-aaaaa-aaabq-cai";
        more_controller_ids = null;
      };
      index_principal = opt principal "2ezyf-hqaaa-aaaar-qb6ga-cai";
    }
  },
)
```

</td>
<td>

```candid
(
  variant {
    Init = record {
      minting_account = record { owner = principal "ljyxk-riaaa-aaaar-qb5mq-cai"; subaccount = null };
      fee_collector_account = opt record {
        owner = principal "ljyxk-riaaa-aaaar-qb5mq-cai";
        subaccount = opt blob "\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\0f\ee";
      };
      transfer_fee = 500 : nat;
      decimals = opt (9 : nat8);
      max_memo_length = opt (80 : nat16);
      token_symbol = "ckDevnetSOL";
      token_name = "ckDevnetSOL";
      metadata = vec {
        record { "icrc1:logo"; variant { Text = "data:image/svg+xml;base64,${LOGO}" } };
      };
      initial_balances = vec {};
      feature_flags = opt record { icrc2 = true };
      archive_options = record {
        num_blocks_to_archive = 1_000 : nat64;
        trigger_threshold = 4_200_000_000 : nat64;
        node_max_memory_size_bytes = opt (3_221_225_472 : nat64);
        max_message_size_bytes = null;
        max_transactions_per_response = null;
        cycles_for_archive_creation = opt (100_000_000_000_000 : nat64);
        controller_id = principal "cmqvo-qqaaa-aaaai-q3waa-cai";
        more_controller_ids = null;
      };
      index_principal = opt principal "2r6ji-gyaaa-aaaar-qb6fq-cai";
    }
  },
)
```

</td>
</tr>
</table>

`LOGO` is the base64 encoding of the token logo, without line breaks.
For production, use `LOGO=$(base64 -i static/images/cksol-token.svg | tr -d '\n')`.
For staging, use `LOGO=$(base64 -i static/images/ckdevnetsol-token.svg | tr -d '\n')`.

The minting account is the minter's default account and the fee collector is the minter's subaccount `0x0fee`.
The transfer fee of 500 lamports follows [Section 3.3.1 of the design](design.md#331-cksol-ledger-fees).
Archiving is effectively disabled by setting `trigger_threshold` to 4,200,000,000 blocks.
The other archive options match those of the ckETH ledger (proposal [126170](https://dashboard.internetcomputer.org/proposal/126170)), so that archives are controlled by the NNS root canister if archiving is enabled by a later upgrade.

## Index

Initialization arguments:

<table>
<tr>
<th>Production</th>
<th>Staging</th>
</tr>
<tr>
<td>

```candid
(
  opt variant {
    Init = record {
      ledger_id = principal "ls5lp-lqaaa-aaaar-qb5oa-cai";
      retrieve_blocks_from_ledger_interval_seconds = null;
    }
  },
)
```

</td>
<td>

```candid
(
  opt variant {
    Init = record {
      ledger_id = principal "la34w-haaaa-aaaar-qb5na-cai";
      retrieve_blocks_from_ledger_interval_seconds = null;
    }
  },
)
```

</td>
</tr>
</table>

The index takes an optional argument, hence the leading `opt`.
Leaving `retrieve_blocks_from_ledger_interval_seconds` unset uses the index's default polling interval.

## Test

### Deposit SOL

The commands below use the staging minter and ledger.
For production, replace the canister IDs with those of the production minter and ledger.

```shell
MINTER=ljyxk-riaaa-aaaar-qb5mq-cai
LEDGER=la34w-haaaa-aaaar-qb5na-cai
OWNER=$(icp identity principal --identity demo)
```

### Get the deposit address

```shell
icp canister call $MINTER get_deposit_address "(record { owner = opt principal \"$OWNER\"; subaccount = null })" --query -n ic
```

Send SOL to the returned address ([faucet](https://faucet.solana.com/)).
The balance of the deposit address must be at least `minimum_deposit_amount`, which `get_minter_info` returns:

```shell
icp canister call $MINTER get_minter_info '()' --query -n ic
```

### Queue the deposit

`deposit_sol` requires `deposit_sol_required_cycles` (1T cycles) to be attached to the call.
Since an identity cannot attach cycles, route the call through a proxy canister that holds cycles:

```shell
icp canister call $MINTER deposit_sol "(record { owner = opt principal \"$OWNER\"; subaccount = null })" \
  --proxy h35ft-riaaa-aaaar-qb37a-cai \
  --cycles 1000000000000 \
  --identity hsm \
  --identity-password-file ~/.config/icp/hsm.pin \
  -n ic
```

The minter sees the proxy canister as the caller, so `owner` must be set explicitly.
Otherwise, the ckSOL would be minted to the proxy canister.
The call returns the deposit ID, e.g. `(variant { Ok = 0 : nat64 })`.

### Check the deposit status

```shell
icp canister call $MINTER deposit_status '(0 : nat64)' --query -n ic
```

The status goes through `Queued`, `Swept`, `Finalized` and `Minted`.
Once minted, the ckSOL balance of the owner is:

```shell
icp canister call $LEDGER icrc1_balance_of "(record { owner = principal \"$OWNER\"; subaccount = null })" --query -n ic
```
