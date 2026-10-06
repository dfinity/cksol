use crate::{
    state::nonce_pool::{DurableNoncePool, NoncePoolError},
    test_fixtures::address,
};
use solana_address::Address;

#[test]
fn should_fail_to_construct_a_pool_with_duplicate_addresses() {
    assert_eq!(
        DurableNoncePool::new([address(1), address(2), address(1)]),
        Err(NoncePoolError::DuplicateAccount(address(1)))
    );
}

#[test]
fn should_fail_to_add_an_address_already_in_the_pool() {
    let mut pool = pool_of([address(1), address(2)]);

    assert_eq!(
        pool.add_accounts([address(3), address(2)]),
        Err(NoncePoolError::DuplicateAccount(address(2)))
    );
}

#[test]
fn should_leave_the_pool_unchanged_when_an_add_fails() {
    let mut pool = pool_of([address(1)]);

    pool.add_accounts([address(2), address(1)])
        .expect_err("adding a duplicate address should fail");

    assert_eq!(pool, pool_of([address(1)]));
}

fn pool_of(addresses: impl IntoIterator<Item = Address>) -> DurableNoncePool {
    DurableNoncePool::new(addresses).expect("the addresses are pairwise distinct")
}
