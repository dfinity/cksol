use solana_address::Address;
use solana_sdk_ids::{
    address_lookup_table, bpf_loader, bpf_loader_deprecated, bpf_loader_upgradeable,
    compute_budget, config, ed25519_program, feature, loader_v4, native_loader, secp256k1_program,
    secp256r1_program, stake, system_program, sysvar, vote, zk_elgamal_proof_program,
    zk_token_proof_program,
};

#[cfg(test)]
mod tests;

/// The account keys reserved by the Solana runtime, vendored from
/// `ReservedAccountKeys::new_all_activated()` in `agave-reserved-account-keys` v3.1.14,
/// which does not build for the wasm32 target.
const RESERVED_ACCOUNT_KEYS: [Address; 31] = [
    address_lookup_table::ID,
    bpf_loader::ID,
    bpf_loader_deprecated::ID,
    bpf_loader_upgradeable::ID,
    compute_budget::ID,
    config::ID,
    ed25519_program::ID,
    feature::ID,
    loader_v4::ID,
    secp256k1_program::ID,
    secp256r1_program::ID,
    stake::config::ID,
    stake::ID,
    system_program::ID,
    vote::ID,
    zk_elgamal_proof_program::ID,
    zk_token_proof_program::ID,
    sysvar::clock::ID,
    sysvar::epoch_rewards::ID,
    sysvar::epoch_schedule::ID,
    sysvar::fees::ID,
    sysvar::instructions::ID,
    sysvar::last_restart_slot::ID,
    sysvar::recent_blockhashes::ID,
    sysvar::rent::ID,
    sysvar::rewards::ID,
    sysvar::slot_hashes::ID,
    sysvar::slot_history::ID,
    sysvar::stake_history::ID,
    native_loader::ID,
    sysvar::ID,
];

pub(super) fn is_reserved_account_key(address: &Address) -> bool {
    RESERVED_ACCOUNT_KEYS.contains(address)
}
