# Proposal to upgrade the ckSOL minter canister

Repository: `https://github.com/dfinity/cksol.git`

Git hash: `263d506523d1e1d67d57c2f8e393364818ea53b7`

New compressed Wasm hash: `fd89fec8cb9e9941db7988131efd7a4acbb04a2be9f091524abd0d1a669ebd96`

Upgrade args hash: `d6e4447c29a9845c0a3129db56d9275b1d02e59c823f4a78ffa5ac7223715ab8`

Target canister: `lh22c-kyaaa-aaaar-qb5nq-cai`

Previous ckSOL minter proposal: https://dashboard.internetcomputer.org/proposal/144313

---

## Motivation

Upgrade the ckSOL minter to version [0.2.0](https://github.com/dfinity/cksol/releases/tag/cksol_minter-v0.2.0) and enable the automatic deposit flow, so that SOL sent to a deposit address, e.g. straight from an exchange, is converted to ckSOL without any further call.

Currently, a user must call `deposit_sol` after sending SOL to their deposit address and attach cycles to the call, which is impractical when the SOL comes from an exchange. With this upgrade:
* `watch_deposit_address` registers the deposit address of the caller's account for about 24 hours, for free. The minter scans the watched addresses on a timer with `getMultipleAccounts`, and queues a deposit reaching `minimum_deposit_amount` exactly as `deposit_sol` would. No cycles have to be attached.
* An automatically detected deposit pays an automatic deposit fee of 0.002 SOL (2,000,000 lamports), deducted from the minted amount (see [Section 3.1.1 of the design](https://github.com/dfinity/cksol/blob/263d506523d1e1d67d57c2f8e393364818ea53b7/docs/src/development/design.md#311-automatic-flow)). The manual flow with `deposit_sol` is unchanged.
* A sweep of automatic deposits that fails or expires puts the addresses back on the watchlist, so that the funds are swept again without any call.
* The watchlist is recorded in the event log before every upgrade, so that registrations survive upgrades.
* New metrics and dashboard entries report the watched addresses, the scans and the collected fees.
* The minter supports [ICRC-21](https://github.com/dfinity/ICRC/blob/main/ICRCs/ICRC-21/ICRC-21.md) consent messages for `withdraw_sol`, so that ICRC-21-aware wallets can show the user what a withdrawal does before it is signed, and lists the standards it supports via `icrc10_supported_standards`.

The automatic flow stays disabled until `automatic_deposit_fee` is set, which this upgrade does (see the upgrade args below). Once the flow is enabled, the minter cannot be downgraded to the previous version, since that version cannot replay the events recorded by the automatic flow: should an issue arise, it has to be fixed by a further upgrade.

The same Wasm has been running on the staging ckSOL minter [`ljyxk-riaaa-aaaar-qb5mq-cai`](https://dashboard.internetcomputer.org/canister/ljyxk-riaaa-aaaar-qb5mq-cai) on Solana Devnet, with the automatic flow enabled with the same fee.


## Release Notes

```
git log --format='%C(auto) %h %s' bb61dcf2045bca4333a732268c1b6d19e113ba93..263d506523d1e1d67d57c2f8e393364818ea53b7 --
263d506 chore: release v0.2.0 (#296)
c3fc148 feat(minter): support ICRC-21 consent messages (#295)
f2c34aa build(deps): Bump rustls from 0.23.45 to 0.23.46 (#289)
a4d1f03 docs: align the automatic deposit flow documentation with the implementation (#292)
d82171b test(minter): cover the automatic deposit flow end to end and document it (#274)
928e5be feat(minter): report the automatic deposit flow in the metrics and on the dashboard (#287)
eafb983 feat(minter): snapshot the watchlist before an upgrade (#285)
dde9f09 feat(minter): record the origin of a queued deposit (#291)
c8dc4d3 feat(minter): re-arm the automatic deposits of a dropped sweep (#284)
a1d7e12 feat(minter): scan the watched deposit addresses on a timer and queue the detected deposits (#283)
475e240 feat(minter): read deposit address balances in batches with the typed getMultipleAccounts (#282)
6c62204 feat(minter): record the automatic part of a queued deposit and deduct its fee from the mint (#281)
5a745c6 feat(minter): add the watch_deposit_address endpoint (#280)
e12b7aa feat(minter): add the watchlist of deposit addresses with its scan schedule (#279)
cae71bb feat(minter): add a bounded map of expiring entries (#278)
79a49f3 feat(minter)!: add the automatic deposit fee parameter (#277)
588d0fc refactor(minter): extract the queueing of a deposit and the in-flight classification from deposit_sol (#276)
183f2d7 docs(design): automatic deposit flow on top of the balance sweep (#272)
2acf44e test: make the concurrent-access guard tests deterministic (#201)
a79f008 test(minter): upgrade a snapshot of the staging minter to the current wasm (#273)
c61d9d3 test(minter): replay the event logs of the deployed minters (#271)
46725dd docs: publish the documentation as an mdbook (#270)
4b0b08c docs: proposals to install the ckSOL minter, ledger and index (#265)
 ```

## Upgrade args

```
git fetch
git checkout 263d506523d1e1d67d57c2f8e393364818ea53b7
didc encode -d minter/cksol_minter.did -t '(MinterArg)' '(variant { Upgrade = record { automatic_deposit_fee = opt (2_000_000 : nat64) } })' | xxd -r -p | sha256sum
```

## Wasm Verification

Verify that the hash of the gzipped WASM matches the proposed hash.

```
git fetch
git checkout 263d506523d1e1d67d57c2f8e393364818ea53b7
"./scripts/docker-build"
sha256sum ./wasms/cksol_minter.wasm.gz
```