# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-10-09

First release of the Candid types of the ckSOL minter's public interface.

### Added

- Deposit types: `GetDepositAddressArgs`, `DepositSolArgs`, `DepositSolId`, `DepositSolStatus` and `DepositSolError`, including the `InsufficientCyclesError` returned when too few cycles are attached ([#3](https://github.com/dfinity/cksol/pull/3), [#5](https://github.com/dfinity/cksol/pull/5), [#25](https://github.com/dfinity/cksol/pull/25), [#32](https://github.com/dfinity/cksol/pull/32), [#208](https://github.com/dfinity/cksol/pull/208), [#210](https://github.com/dfinity/cksol/pull/210), [#213](https://github.com/dfinity/cksol/pull/213), [#218](https://github.com/dfinity/cksol/pull/218), [#220](https://github.com/dfinity/cksol/pull/220), [#222](https://github.com/dfinity/cksol/pull/222))
- Withdrawal types: `WithdrawSolArgs`, `WithdrawSolOk`, `WithdrawSolError`, `WithdrawSolStatusArgs`, `WithdrawSolStatus` and `TxFinalizedStatus`, with an `InvalidDestination` error for destinations the minter cannot pay out to ([#9](https://github.com/dfinity/cksol/pull/9), [#13](https://github.com/dfinity/cksol/pull/13), [#18](https://github.com/dfinity/cksol/pull/18), [#29](https://github.com/dfinity/cksol/pull/29), [#76](https://github.com/dfinity/cksol/pull/76), [#109](https://github.com/dfinity/cksol/pull/109), [#243](https://github.com/dfinity/cksol/pull/243), [#262](https://github.com/dfinity/cksol/pull/262))
- `MinterInfo`, returned by `get_minter_info`, with the minter's fees, minimum amounts, balance, main address and nonce accounts ([#10](https://github.com/dfinity/cksol/pull/10), [#86](https://github.com/dfinity/cksol/pull/86), [#241](https://github.com/dfinity/cksol/pull/241))
- Ledger memo types: `Memo`, `MintMemo` and `BurnMemo`, encoded in CBOR and bounded by `MAX_SERIALIZED_MEMO_BYTES` ([#18](https://github.com/dfinity/cksol/pull/18), [#22](https://github.com/dfinity/cksol/pull/22), [#170](https://github.com/dfinity/cksol/pull/170))
- Re-exports of `Lamport`, `Address` and `Signature`, and the `LedgerMintIndex` and `LedgerBurnIndex` aliases ([#1](https://github.com/dfinity/cksol/pull/1), [#153](https://github.com/dfinity/cksol/pull/153))

[0.1.0]: https://github.com/dfinity/cksol/releases/tag/cksol-types-v0.1.0
