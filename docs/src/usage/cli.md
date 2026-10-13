# Command Line Interface

You can interact with the ckSOL minter using [`icp-cli`](https://github.com/dfinity/icp-cli). Pass `-e prod` (or `-e staging`) to target the corresponding environment defined in [`icp.yaml`](https://github.com/dfinity/cksol/blob/main/icp.yaml).

## Get minter info

Query fees, minimum amounts, and the current minter balance:

```sh
icp canister call -e prod cksol_minter get_minter_info '()'
```

## Get your deposit address

Returns the Solana address you should send SOL to in order to deposit. When `owner` is `null`, it defaults to your calling identity's principal:

```sh
icp canister call -e prod cksol_minter get_deposit_address \
  '(record { owner = null; subaccount = null })'
```

## Notify the minter of a deposit

After sending SOL to your deposit address, call `deposit_sol` to queue the deposit address for a sweep to the minter's main account. Pass the same `owner`/`subaccount` used when calling `get_deposit_address`.

This call requires attaching cycles — check the required amount via `get_minter_info` (`deposit_sol_required_cycles` field); unused cycles are refunded. Since only canisters can attach cycles to a call, route it through a [proxy canister](https://cli.internetcomputer.org/0.2/guides/proxy-canister/) that you control and that holds cycles (you can [convert ICP to cycles](https://cli.internetcomputer.org/0.2/guides/tokens-and-cycles/#converting-icp-to-cycles) to fund it). Replace `<PROXY_PRINCIPAL>` with the proxy canister and `<CYCLES>` with at least `deposit_sol_required_cycles`:

```sh
icp canister call -e prod cksol_minter deposit_sol \
  '(record { owner = opt principal "<YOUR_PRINCIPAL>"; subaccount = null })' \
  --proxy <PROXY_PRINCIPAL> \
  --cycles <CYCLES>
```

> [!NOTE]
> The minter sees the proxy canister as the caller, so `owner` must be set explicitly: replace `<YOUR_PRINCIPAL>` with the owner of your deposit address, which is your identity's principal (`icp identity principal`) if you called `get_deposit_address` with `owner = null`. With `owner = null`, the minter would check the deposit address of the proxy canister instead.

A successful response returns the deposit id, which you can use to track the deposit:

```
(variant { Ok = 42 : nat64 })
```

## Check a deposit status

After calling `deposit_sol`, track the deposit using the deposit id returned in the response. The status moves through `Queued` → `Swept` → `Finalized` → `Minted` as the minter sweeps the deposit address and mints ckSOL:

```sh
icp canister call -e prod cksol_minter deposit_sol_status '(42 : nat64)' --query
```

## Submit a withdrawal request

Burns ckSOL from your ledger account and initiates a transfer of the equivalent SOL to the given Solana address. Replace `<SOLANA_ADDRESS>` with the destination address and `<AMOUNT>` with the amount in lamports. The optional `from_subaccount` field defaults to `null` (the default subaccount):

```sh
icp canister call -e prod cksol_minter withdraw_sol \
  '(record { address = "<SOLANA_ADDRESS>"; amount = <AMOUNT>; from_subaccount = null })'
```

A successful response returns the burn block index, which you can use to track the withdrawal:

```
(variant { Ok = record { block_index = 42 } })
```

## Check a withdrawal status

After calling `withdraw_sol`, track the status using the `block_index` returned in the response:

```sh
icp canister call -e prod cksol_minter withdraw_sol_status \
  '(record { block_index = 42 })'
```
