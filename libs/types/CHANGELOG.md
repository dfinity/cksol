# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-10-09

### Added

- Add a pool of durable nonce accounts ([#241](https://github.com/dfinity/cksol/pull/241))
- Add deposit_sol and deposit_status endpoint skeletons ([#208](https://github.com/dfinity/cksol/pull/208))
- Add `update_balance` endpoint for automated deposit monitoring ([#136](https://github.com/dfinity/cksol/pull/136))
- Add `deposit_amount` to `DepositStatus::Processing` ([#45](https://github.com/dfinity/cksol/pull/45))
- Add parameter for required cycles for `update_balance` calls ([#32](https://github.com/dfinity/cksol/pull/32))
- DEFI-2671: add withdrawal fee ([#29](https://github.com/dfinity/cksol/pull/29))
- Add events for deposit flow ([#26](https://github.com/dfinity/cksol/pull/26))
- Add minimum deposit amount ([#25](https://github.com/dfinity/cksol/pull/25))
- Add concurrent access guard to `update_balance` endpoint ([#17](https://github.com/dfinity/cksol/pull/17))
- DEFI-2671: add minimum_withdrawal_amount parameter ([#13](https://github.com/dfinity/cksol/pull/13))
- Add deposit flow skeleton ([#5](https://github.com/dfinity/cksol/pull/5))

### Changed

- Name the withdrawal and status endpoints after the asset ([#262](https://github.com/dfinity/cksol/pull/262))
- Validate withdrawal destinations ([#243](https://github.com/dfinity/cksol/pull/243))
- Mint the pending deposits of credited sweeps on a timer ([#220](https://github.com/dfinity/cksol/pull/220))
- Drop failed sweeps and quarantine sweeps with mismatching metadata ([#218](https://github.com/dfinity/cksol/pull/218))
- Credit the deposits of a finalized sweep from the transaction metadata ([#222](https://github.com/dfinity/cksol/pull/222))
- Sweep queued deposits to the main address on a timer ([#213](https://github.com/dfinity/cksol/pull/213))
- Queue deposits in the state under sequence numbers ([#210](https://github.com/dfinity/cksol/pull/210))
- Revert "add update_balance endpoint for automated deposit monitoring" ([#136](https://github.com/dfinity/cksol/pull/136)) ([#199](https://github.com/dfinity/cksol/pull/199))
- Revert "remove owner field from UpdateBalanceArgs" ([#169](https://github.com/dfinity/cksol/pull/169)) ([#195](https://github.com/dfinity/cksol/pull/195))
- Rename `deposit_fee` to `manual_deposit_fee` and add `automated_deposit_fee` ([#133](https://github.com/dfinity/cksol/pull/133))
- Rename `update_balance` to `process_deposit` ([#141](https://github.com/dfinity/cksol/pull/141))
- Improve `withdraw` function and restructure `WithdrawalRequest` ([#109](https://github.com/dfinity/cksol/pull/109))
- Charge deposit consolidation fee in cycles during `update_balance` ([#108](https://github.com/dfinity/cksol/pull/108))
- Handle ledger errors explicitly in mint and burn calls ([#112](https://github.com/dfinity/cksol/pull/112))
- Rename `withdraw_sol` to `withdraw` across the codebase ([#110](https://github.com/dfinity/cksol/pull/110))
- Track minter balance through state transitions ([#86](https://github.com/dfinity/cksol/pull/86))
- Update withdrawal status on transaction finalization ([#76](https://github.com/dfinity/cksol/pull/76))
- Use `DepositId` instead of `Signature` in `DepositStatus` ([#89](https://github.com/dfinity/cksol/pull/89))
- DEFI-2671: schedule task for processing withdrawals ([#48](https://github.com/dfinity/cksol/pull/48))
- Charge users for `update_balance` endpoint ([#28](https://github.com/dfinity/cksol/pull/28))
- Submit consolidation transactions ([#49](https://github.com/dfinity/cksol/pull/49))
- Retry minting when calling `update_balance` again ([#36](https://github.com/dfinity/cksol/pull/36))
- DEFI-2671: burn funds for withdrawal ([#18](https://github.com/dfinity/cksol/pull/18))
- Mint on valid call to `update_balance` ([#22](https://github.com/dfinity/cksol/pull/22))
- Fetch transaction in `update_balance` endpoint ([#14](https://github.com/dfinity/cksol/pull/14))
- DEFI-2671: withdrawal workflow endpoints skeleton ([#9](https://github.com/dfinity/cksol/pull/9))
- `get_minter_info` endpoint and logging infrastructure ([#10](https://github.com/dfinity/cksol/pull/10))
- Initialization and upgrade endpoints ([#7](https://github.com/dfinity/cksol/pull/7))
- DEFI-2640: Endpoint for deriving Solana deposit address ([#3](https://github.com/dfinity/cksol/pull/3))
- Initial cargo workspace and build pipeline ([#1](https://github.com/dfinity/cksol/pull/1))

### Fixed

- Issues found while deploying to staging ([#263](https://github.com/dfinity/cksol/pull/263))
- CBOR field index for `Burn` memo inner tuple ([#170](https://github.com/dfinity/cksol/pull/170))
- Clean up Candid interface and standardize naming conventions ([#153](https://github.com/dfinity/cksol/pull/153))
- Setup `PocketIc` tests ([#6](https://github.com/dfinity/cksol/pull/6))

### Removed

- Remove the signature-based deposit flow ([#221](https://github.com/dfinity/cksol/pull/221))
- Remove owner field from `UpdateBalanceArgs` ([#169](https://github.com/dfinity/cksol/pull/169))


