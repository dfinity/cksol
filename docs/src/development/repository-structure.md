# Repository Structure

```
.
├── minter/                  # ckSOL minter canister
│   ├── src/
│   │   ├── address/         # Deposit address derivation
│   │   ├── dashboard/       # HTTP dashboard
│   │   ├── lifecycle.rs     # Canister init/upgrade and event log
│   │   ├── metrics.rs       # Prometheus metrics
│   │   ├── monitor/         # Transaction monitoring
│   │   ├── state/           # Minter state and event sourcing
│   │   ├── deposit/sweep/   # Deposit sweeping and minting
│   │   ├── withdraw/        # Withdrawal processing
│   │   └── ...
│   └── cksol_minter.did     # Candid interface
├── libs/
│   ├── types/               # Public ckSOL types (cksol-types crate)
│   └── types-internal/      # Internal types and event definitions
├── integration_tests/       # End-to-end tests using PocketIC
├── docs/
│   ├── book.toml            # mdbook configuration
│   └── src/                 # This documentation
└── scripts/
    └── build                # Build script for the minter Wasm
```
