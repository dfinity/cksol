# Deployment

## Minter's address on Solana

Given a canister ID, the address controlled by the minter can be derived offline without installing any code.
See `should_derive_mainnet_minter_addresses_offline` for an example.

| Environment | Canister ID                                                                                                  | Solana address                                                                                                                                    |
|-------------|--------------------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------|
| Production  | [`lh22c-kyaaa-aaaar-qb5nq-cai`](https://dashboard.internetcomputer.org/canister/lh22c-kyaaa-aaaar-qb5nq-cai) | [`GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax`](https://explorer.solana.com/address/GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax)                |
| Staging     | [`ljyxk-riaaa-aaaar-qb5mq-cai`](https://dashboard.internetcomputer.org/canister/ljyxk-riaaa-aaaar-qb5mq-cai) | [`Br8eRkeya8hy3sHCYqtWqGeNNZ349aPSUKGVYFWKKer1`](https://explorer.solana.com/address/Br8eRkeya8hy3sHCYqtWqGeNNZ349aPSUKGVYFWKKer1?cluster=devnet) |

### Create nonce accounts

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
        record { "icrc1:logo"; variant { Text = "data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzYwIiBoZWlnaHQ9IjM2MCIgdmlld0JveD0iMCAwIDM2MCAzNjAiIGZpbGw9Im5vbmUiIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyI+CjxnIGNsaXAtcGF0aD0idXJsKCNjbGlwMF8xMDIzXzQ5KSI+CjxwYXRoIGQ9Ik0xODAgMEMyNzkuNCAwIDM2MCA4MC42IDM2MCAxODBDMzYwIDI3OS40IDI3OS40IDM2MCAxODAgMzYwQzgwLjYgMzYwIDAgMjc5LjQgMCAxODBDMCA4MC42IDgwLjYgMCAxODAgMFoiIGZpbGw9IiMzQjAwQjkiLz4KPHBhdGggZmlsbC1ydWxlPSJldmVub2RkIiBjbGlwLXJ1bGU9ImV2ZW5vZGQiIGQ9Ik00MC4zOTk4IDE5MC40QzQ1LjM5OTggMjU5LjQgMTAwLjYgMzE0LjYgMTY5LjYgMzE5LjZWMzM1LjJDOTEuOTk5OCAzMzAgMjkuOTk5OCAyNjggMjQuNzk5OCAxOTAuNEg0MC4zOTk4WiIgZmlsbD0idXJsKCNwYWludDBfbGluZWFyXzEwMjNfNDkpIi8+CjxwYXRoIGZpbGwtcnVsZT0iZXZlbm9kZCIgY2xpcC1ydWxlPSJldmVub2RkIiBkPSJNMTY5LjYgNDAuNEMxMDAuNiA0NS40IDQ1LjM5OTggMTAwLjYgNDAuMzk5OCAxNjkuNkgyNC43OTk4QzI5Ljc5OTggOTIgOTEuOTk5OCAyOS44IDE2OS42IDI0LjhWNDAuNFoiIGZpbGw9IiMyOUFCRTIiLz4KPHBhdGggZmlsbC1ydWxlPSJldmVub2RkIiBjbGlwLXJ1bGU9ImV2ZW5vZGQiIGQ9Ik0zMTkuNiAxNjkuNEMzMTQuNiAxMDAuNCAyNTkuNCA0NS4yIDE5MC40IDQwLjJWMjQuNkMyNjggMjkuOCAzMzAuMiA5MS44IDMzNS4yIDE2OS40SDMxOS42WiIgZmlsbD0idXJsKCNwYWludDFfbGluZWFyXzEwMjNfNDkpIi8+CjxwYXRoIGZpbGwtcnVsZT0iZXZlbm9kZCIgY2xpcC1ydWxlPSJldmVub2RkIiBkPSJNMTkwLjQgMzE5LjZDMjU5LjQgMzE0LjYgMzE0LjYgMjU5LjQgMzE5LjYgMTkwLjRIMzM1LjJDMzMwLjIgMjY4IDI2OCAzMzAgMTkwLjQgMzM1LjJWMzE5LjZaIiBmaWxsPSIjMjlBQkUyIi8+CjxnIGNsaXAtcGF0aD0idXJsKCNjbGlwMV8xMDIzXzQ5KSI+CjxwYXRoIGQ9Ik0yNjQuMTI1IDIyMi43ODFMMjM2LjA2MSAyNTIuMTAyQzIzNS40NTEgMjUyLjczOCAyMzQuNzEzIDI1My4yNDYgMjMzLjg5MyAyNTMuNTkzQzIzMy4wNzIgMjUzLjk0IDIzMi4xODcgMjU0LjExOSAyMzEuMjkzIDI1NC4xMTlIOTguMjU4Qzk3LjYyMzIgMjU0LjExOSA5Ny4wMDIyIDI1My45MzggOTYuNDcxNCAyNTMuNTk5Qzk1Ljk0MDYgMjUzLjI2IDk1LjUyMyAyNTIuNzc3IDk1LjI3IDI1Mi4yMUM5NS4wMTcgMjUxLjY0MyA5NC45Mzk1IDI1MS4wMTYgOTUuMDQ3MiAyNTAuNDA3Qzk1LjE1NDggMjQ5Ljc5NyA5NS40NDI5IDI0OS4yMzIgOTUuODc2IDI0OC43NzlMMTIzLjk2MSAyMTkuNDU5QzEyNC41NjkgMjE4LjgyNCAxMjUuMzA1IDIxOC4zMTcgMTI2LjEyMiAyMTcuOTdDMTI2Ljk0IDIxNy42MjMgMTI3LjgyMiAyMTcuNDQzIDEyOC43MTQgMjE3LjQ0MkgyNjEuNzQyQzI2Mi4zNzcgMjE3LjQ0MiAyNjIuOTk4IDIxNy42MjMgMjYzLjUyOSAyMTcuOTYyQzI2NC4wNTkgMjE4LjMwMSAyNjQuNDc3IDIxOC43ODQgMjY0LjczMSAyMTkuMzUxQzI2NC45ODMgMjE5LjkxOCAyNjUuMDYxIDIyMC41NDQgMjY0Ljk1MyAyMjEuMTU0QzI2NC44NDUgMjIxLjc2MyAyNjQuNTU3IDIyMi4zMjkgMjY0LjEyNSAyMjIuNzgxWk0yMzYuMDYxIDE2My43MzhDMjM1LjQ1MSAxNjMuMTAxIDIzNC43MTMgMTYyLjU5MyAyMzMuODkzIDE2Mi4yNDZDMjMzLjA3MiAxNjEuODk5IDIzMi4xODcgMTYxLjcyIDIzMS4yOTMgMTYxLjcyMUg5OC4yNThDOTcuNjIzMiAxNjEuNzIxIDk3LjAwMjIgMTYxLjkwMiA5Ni40NzE0IDE2Mi4yNDFDOTUuOTQwNiAxNjIuNTggOTUuNTIzIDE2My4wNjMgOTUuMjcgMTYzLjYzQzk1LjAxNyAxNjQuMTk3IDk0LjkzOTUgMTY0LjgyNCA5NS4wNDcyIDE2NS40MzNDOTUuMTU0OCAxNjYuMDQyIDk1LjQ0MjkgMTY2LjYwOCA5NS44NzYgMTY3LjA2TDEyMy45NjEgMTk2LjM4MUMxMjQuNTY5IDE5Ny4wMTYgMTI1LjMwNSAxOTcuNTIzIDEyNi4xMjIgMTk3Ljg3QzEyNi45NCAxOTguMjE3IDEyNy44MjIgMTk4LjM5NyAxMjguNzE0IDE5OC4zOThIMjYxLjc0MkMyNjIuMzc3IDE5OC4zOTggMjYyLjk5OCAxOTguMjE3IDI2My41MjkgMTk3Ljg3OEMyNjQuMDU5IDE5Ny41MzkgMjY0LjQ3NyAxOTcuMDU2IDI2NC43MzEgMTk2LjQ4OUMyNjQuOTgzIDE5NS45MjIgMjY1LjA2MSAxOTUuMjk1IDI2NC45NTMgMTk0LjY4NkMyNjQuODQ1IDE5NC4wNzYgMjY0LjU1NyAxOTMuNTExIDI2NC4xMjUgMTkzLjA1OUwyMzYuMDYxIDE2My43MzhaTTk4LjI1OCAxNDIuNjc3SDIzMS4yOTNDMjMyLjE4NyAxNDIuNjc3IDIzMy4wNzIgMTQyLjQ5OSAyMzMuODkzIDE0Mi4xNTJDMjM0LjcxMyAxNDEuODA1IDIzNS40NTEgMTQxLjI5NyAyMzYuMDYxIDE0MC42NkwyNjQuMTI1IDExMS4zMzlDMjY0LjU1NyAxMTAuODg3IDI2NC44NDUgMTEwLjMyMiAyNjQuOTUzIDEwOS43MTJDMjY1LjA2MSAxMDkuMTAzIDI2NC45ODMgMTA4LjQ3NiAyNjQuNzMxIDEwNy45MDlDMjY0LjQ3NyAxMDcuMzQyIDI2NC4wNTkgMTA2Ljg1OSAyNjMuNTI5IDEwNi41MkMyNjIuOTk4IDEwNi4xODEgMjYyLjM3NyAxMDYgMjYxLjc0MiAxMDZIMTI4LjcxNEMxMjcuODIyIDEwNi4wMDEgMTI2Ljk0IDEwNi4xODEgMTI2LjEyMiAxMDYuNTI4QzEyNS4zMDUgMTA2Ljg3NSAxMjQuNTY5IDEwNy4zODIgMTIzLjk2MSAxMDguMDE3TDk1Ljg4MzIgMTM3LjMzOEM5NS40NTA2IDEzNy43ODkgOTUuMTYyNiAxMzguMzU0IDk1LjA1NDcgMTM4Ljk2M0M5NC45NDY4IDEzOS41NzIgOTUuMDIzNyAxNDAuMTk4IDk1LjI3NTggMTQwLjc2NUM5NS41Mjc5IDE0MS4zMzIgOTUuOTQ0NCAxNDEuODE1IDk2LjQ3NDEgMTQyLjE1NEM5Ny4wMDM5IDE0Mi40OTQgOTcuNjIzOCAxNDIuNjc2IDk4LjI1OCAxNDIuNjc3WiIgZmlsbD0id2hpdGUiLz4KPC9nPgo8L2c+CjxkZWZzPgo8bGluZWFyR3JhZGllbnQgaWQ9InBhaW50MF9saW5lYXJfMTAyM180OSIgeDE9IjEzMC43MiIgeTE9IjMwNC4xMiIgeDI9IjMzLjQ3OTgiIHkyPSIyMjIuMjIiIGdyYWRpZW50VW5pdHM9InVzZXJTcGFjZU9uVXNlIj4KPHN0b3Agb2Zmc2V0PSIwLjIxIiBzdG9wLWNvbG9yPSIjRUQxRTc5Ii8+CjxzdG9wIG9mZnNldD0iMSIgc3RvcC1jb2xvcj0iIzUyMjc4NSIvPgo8L2xpbmVhckdyYWRpZW50Pgo8bGluZWFyR3JhZGllbnQgaWQ9InBhaW50MV9saW5lYXJfMTAyM180OSIgeDE9IjMwOS4zMiIgeTE9IjEyMy4wNiIgeDI9IjIxMi4wOCIgeTI9IjQxLjE2IiBncmFkaWVudFVuaXRzPSJ1c2VyU3BhY2VPblVzZSI+CjxzdG9wIG9mZnNldD0iMC4yMSIgc3RvcC1jb2xvcj0iI0YxNUEyNCIvPgo8c3RvcCBvZmZzZXQ9IjAuNjgiIHN0b3AtY29sb3I9IiNGQkIwM0IiLz4KPC9saW5lYXJHcmFkaWVudD4KPGNsaXBQYXRoIGlkPSJjbGlwMF8xMDIzXzQ5Ij4KPHJlY3Qgd2lkdGg9IjM2MCIgaGVpZ2h0PSIzNjAiIGZpbGw9IndoaXRlIi8+CjwvY2xpcFBhdGg+CjxjbGlwUGF0aCBpZD0iY2xpcDFfMTAyM180OSI+CjxyZWN0IHdpZHRoPSIxNzAiIGhlaWdodD0iMTQ4LjExOSIgZmlsbD0id2hpdGUiIHRyYW5zZm9ybT0idHJhbnNsYXRlKDk1IDEwNikiLz4KPC9jbGlwUGF0aD4KPC9kZWZzPgo8L3N2Zz4K" } };
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
      token_symbol = "ckSOL";
      token_name = "ckSOL";
      metadata = vec {
        record { "icrc1:logo"; variant { Text = "data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMzYwIiBoZWlnaHQ9IjM2MCIgdmlld0JveD0iMCAwIDM2MCAzNjAiIGZpbGw9Im5vbmUiIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyI+CjxnIGNsaXAtcGF0aD0idXJsKCNjbGlwMF8xMDIzXzQ5KSI+CjxwYXRoIGQ9Ik0xODAgMEMyNzkuNCAwIDM2MCA4MC42IDM2MCAxODBDMzYwIDI3OS40IDI3OS40IDM2MCAxODAgMzYwQzgwLjYgMzYwIDAgMjc5LjQgMCAxODBDMCA4MC42IDgwLjYgMCAxODAgMFoiIGZpbGw9IiMzQjAwQjkiLz4KPHBhdGggZmlsbC1ydWxlPSJldmVub2RkIiBjbGlwLXJ1bGU9ImV2ZW5vZGQiIGQ9Ik00MC4zOTk4IDE5MC40QzQ1LjM5OTggMjU5LjQgMTAwLjYgMzE0LjYgMTY5LjYgMzE5LjZWMzM1LjJDOTEuOTk5OCAzMzAgMjkuOTk5OCAyNjggMjQuNzk5OCAxOTAuNEg0MC4zOTk4WiIgZmlsbD0idXJsKCNwYWludDBfbGluZWFyXzEwMjNfNDkpIi8+CjxwYXRoIGZpbGwtcnVsZT0iZXZlbm9kZCIgY2xpcC1ydWxlPSJldmVub2RkIiBkPSJNMTY5LjYgNDAuNEMxMDAuNiA0NS40IDQ1LjM5OTggMTAwLjYgNDAuMzk5OCAxNjkuNkgyNC43OTk4QzI5Ljc5OTggOTIgOTEuOTk5OCAyOS44IDE2OS42IDI0LjhWNDAuNFoiIGZpbGw9IiMyOUFCRTIiLz4KPHBhdGggZmlsbC1ydWxlPSJldmVub2RkIiBjbGlwLXJ1bGU9ImV2ZW5vZGQiIGQ9Ik0zMTkuNiAxNjkuNEMzMTQuNiAxMDAuNCAyNTkuNCA0NS4yIDE5MC40IDQwLjJWMjQuNkMyNjggMjkuOCAzMzAuMiA5MS44IDMzNS4yIDE2OS40SDMxOS42WiIgZmlsbD0idXJsKCNwYWludDFfbGluZWFyXzEwMjNfNDkpIi8+CjxwYXRoIGZpbGwtcnVsZT0iZXZlbm9kZCIgY2xpcC1ydWxlPSJldmVub2RkIiBkPSJNMTkwLjQgMzE5LjZDMjU5LjQgMzE0LjYgMzE0LjYgMjU5LjQgMzE5LjYgMTkwLjRIMzM1LjJDMzMwLjIgMjY4IDI2OCAzMzAgMTkwLjQgMzM1LjJWMzE5LjZaIiBmaWxsPSIjMjlBQkUyIi8+CjxnIGNsaXAtcGF0aD0idXJsKCNjbGlwMV8xMDIzXzQ5KSI+CjxwYXRoIGQ9Ik0yNjQuMTI1IDIyMi43ODFMMjM2LjA2MSAyNTIuMTAyQzIzNS40NTEgMjUyLjczOCAyMzQuNzEzIDI1My4yNDYgMjMzLjg5MyAyNTMuNTkzQzIzMy4wNzIgMjUzLjk0IDIzMi4xODcgMjU0LjExOSAyMzEuMjkzIDI1NC4xMTlIOTguMjU4Qzk3LjYyMzIgMjU0LjExOSA5Ny4wMDIyIDI1My45MzggOTYuNDcxNCAyNTMuNTk5Qzk1Ljk0MDYgMjUzLjI2IDk1LjUyMyAyNTIuNzc3IDk1LjI3IDI1Mi4yMUM5NS4wMTcgMjUxLjY0MyA5NC45Mzk1IDI1MS4wMTYgOTUuMDQ3MiAyNTAuNDA3Qzk1LjE1NDggMjQ5Ljc5NyA5NS40NDI5IDI0OS4yMzIgOTUuODc2IDI0OC43NzlMMTIzLjk2MSAyMTkuNDU5QzEyNC41NjkgMjE4LjgyNCAxMjUuMzA1IDIxOC4zMTcgMTI2LjEyMiAyMTcuOTdDMTI2Ljk0IDIxNy42MjMgMTI3LjgyMiAyMTcuNDQzIDEyOC43MTQgMjE3LjQ0MkgyNjEuNzQyQzI2Mi4zNzcgMjE3LjQ0MiAyNjIuOTk4IDIxNy42MjMgMjYzLjUyOSAyMTcuOTYyQzI2NC4wNTkgMjE4LjMwMSAyNjQuNDc3IDIxOC43ODQgMjY0LjczMSAyMTkuMzUxQzI2NC45ODMgMjE5LjkxOCAyNjUuMDYxIDIyMC41NDQgMjY0Ljk1MyAyMjEuMTU0QzI2NC44NDUgMjIxLjc2MyAyNjQuNTU3IDIyMi4zMjkgMjY0LjEyNSAyMjIuNzgxWk0yMzYuMDYxIDE2My43MzhDMjM1LjQ1MSAxNjMuMTAxIDIzNC43MTMgMTYyLjU5MyAyMzMuODkzIDE2Mi4yNDZDMjMzLjA3MiAxNjEuODk5IDIzMi4xODcgMTYxLjcyIDIzMS4yOTMgMTYxLjcyMUg5OC4yNThDOTcuNjIzMiAxNjEuNzIxIDk3LjAwMjIgMTYxLjkwMiA5Ni40NzE0IDE2Mi4yNDFDOTUuOTQwNiAxNjIuNTggOTUuNTIzIDE2My4wNjMgOTUuMjcgMTYzLjYzQzk1LjAxNyAxNjQuMTk3IDk0LjkzOTUgMTY0LjgyNCA5NS4wNDcyIDE2NS40MzNDOTUuMTU0OCAxNjYuMDQyIDk1LjQ0MjkgMTY2LjYwOCA5NS44NzYgMTY3LjA2TDEyMy45NjEgMTk2LjM4MUMxMjQuNTY5IDE5Ny4wMTYgMTI1LjMwNSAxOTcuNTIzIDEyNi4xMjIgMTk3Ljg3QzEyNi45NCAxOTguMjE3IDEyNy44MjIgMTk4LjM5NyAxMjguNzE0IDE5OC4zOThIMjYxLjc0MkMyNjIuMzc3IDE5OC4zOTggMjYyLjk5OCAxOTguMjE3IDI2My41MjkgMTk3Ljg3OEMyNjQuMDU5IDE5Ny41MzkgMjY0LjQ3NyAxOTcuMDU2IDI2NC43MzEgMTk2LjQ4OUMyNjQuOTgzIDE5NS45MjIgMjY1LjA2MSAxOTUuMjk1IDI2NC45NTMgMTk0LjY4NkMyNjQuODQ1IDE5NC4wNzYgMjY0LjU1NyAxOTMuNTExIDI2NC4xMjUgMTkzLjA1OUwyMzYuMDYxIDE2My43MzhaTTk4LjI1OCAxNDIuNjc3SDIzMS4yOTNDMjMyLjE4NyAxNDIuNjc3IDIzMy4wNzIgMTQyLjQ5OSAyMzMuODkzIDE0Mi4xNTJDMjM0LjcxMyAxNDEuODA1IDIzNS40NTEgMTQxLjI5NyAyMzYuMDYxIDE0MC42NkwyNjQuMTI1IDExMS4zMzlDMjY0LjU1NyAxMTAuODg3IDI2NC44NDUgMTEwLjMyMiAyNjQuOTUzIDEwOS43MTJDMjY1LjA2MSAxMDkuMTAzIDI2NC45ODMgMTA4LjQ3NiAyNjQuNzMxIDEwNy45MDlDMjY0LjQ3NyAxMDcuMzQyIDI2NC4wNTkgMTA2Ljg1OSAyNjMuNTI5IDEwNi41MkMyNjIuOTk4IDEwNi4xODEgMjYyLjM3NyAxMDYgMjYxLjc0MiAxMDZIMTI4LjcxNEMxMjcuODIyIDEwNi4wMDEgMTI2Ljk0IDEwNi4xODEgMTI2LjEyMiAxMDYuNTI4QzEyNS4zMDUgMTA2Ljg3NSAxMjQuNTY5IDEwNy4zODIgMTIzLjk2MSAxMDguMDE3TDk1Ljg4MzIgMTM3LjMzOEM5NS40NTA2IDEzNy43ODkgOTUuMTYyNiAxMzguMzU0IDk1LjA1NDcgMTM4Ljk2M0M5NC45NDY4IDEzOS41NzIgOTUuMDIzNyAxNDAuMTk4IDk1LjI3NTggMTQwLjc2NUM5NS41Mjc5IDE0MS4zMzIgOTUuOTQ0NCAxNDEuODE1IDk2LjQ3NDEgMTQyLjE1NEM5Ny4wMDM5IDE0Mi40OTQgOTcuNjIzOCAxNDIuNjc2IDk4LjI1OCAxNDIuNjc3WiIgZmlsbD0id2hpdGUiLz4KPC9nPgo8L2c+CjxkZWZzPgo8bGluZWFyR3JhZGllbnQgaWQ9InBhaW50MF9saW5lYXJfMTAyM180OSIgeDE9IjEzMC43MiIgeTE9IjMwNC4xMiIgeDI9IjMzLjQ3OTgiIHkyPSIyMjIuMjIiIGdyYWRpZW50VW5pdHM9InVzZXJTcGFjZU9uVXNlIj4KPHN0b3Agb2Zmc2V0PSIwLjIxIiBzdG9wLWNvbG9yPSIjRUQxRTc5Ii8+CjxzdG9wIG9mZnNldD0iMSIgc3RvcC1jb2xvcj0iIzUyMjc4NSIvPgo8L2xpbmVhckdyYWRpZW50Pgo8bGluZWFyR3JhZGllbnQgaWQ9InBhaW50MV9saW5lYXJfMTAyM180OSIgeDE9IjMwOS4zMiIgeTE9IjEyMy4wNiIgeDI9IjIxMi4wOCIgeTI9IjQxLjE2IiBncmFkaWVudFVuaXRzPSJ1c2VyU3BhY2VPblVzZSI+CjxzdG9wIG9mZnNldD0iMC4yMSIgc3RvcC1jb2xvcj0iI0YxNUEyNCIvPgo8c3RvcCBvZmZzZXQ9IjAuNjgiIHN0b3AtY29sb3I9IiNGQkIwM0IiLz4KPC9saW5lYXJHcmFkaWVudD4KPGNsaXBQYXRoIGlkPSJjbGlwMF8xMDIzXzQ5Ij4KPHJlY3Qgd2lkdGg9IjM2MCIgaGVpZ2h0PSIzNjAiIGZpbGw9IndoaXRlIi8+CjwvY2xpcFBhdGg+CjxjbGlwUGF0aCBpZD0iY2xpcDFfMTAyM180OSI+CjxyZWN0IHdpZHRoPSIxNzAiIGhlaWdodD0iMTQ4LjExOSIgZmlsbD0id2hpdGUiIHRyYW5zZm9ybT0idHJhbnNsYXRlKDk1IDEwNikiLz4KPC9jbGlwUGF0aD4KPC9kZWZzPgo8L3N2Zz4K" } };
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
      index_principal = opt principal "2r6ji-gyaaa-aaaar-qb6fq-cai";
    }
  },
)
```

</td>
</tr>
</table>

The minting account is the minter's default account and the fee collector is the minter's subaccount `0x0fee`.
The transfer fee of 500 lamports follows [Section 3.3.1 of the design](design.md#331-cksol-ledger-fees).
Archiving is effectively disabled by setting `trigger_threshold` to 4,200,000,000 blocks.
The other archive options match those of the ckETH ledger (proposal [126170](https://dashboard.internetcomputer.org/proposal/126170)), so that archives are controlled by the NNS root canister if archiving is enabled by a later upgrade.
