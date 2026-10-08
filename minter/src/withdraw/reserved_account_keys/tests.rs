use crate::withdraw::reserved_account_keys::RESERVED_ACCOUNT_KEYS;
use agave_reserved_account_keys::ReservedAccountKeys;
use solana_address::Address;
use std::collections::BTreeSet;

#[test]
fn should_match_the_set_of_the_reserved_account_keys_crate() {
    let vendored: BTreeSet<Address> = RESERVED_ACCOUNT_KEYS.into_iter().collect();

    let upstream: BTreeSet<Address> = ReservedAccountKeys::new_all_activated()
        .active
        .into_iter()
        .map(|key| Address::from(key.to_bytes()))
        .collect();

    assert_eq!(vendored, upstream);
}
