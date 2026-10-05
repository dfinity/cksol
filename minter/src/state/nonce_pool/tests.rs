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
fn should_fail_to_remove_an_address_not_in_the_pool() {
    let mut pool = pool_of([address(1)]);

    assert_eq!(
        pool.remove_accounts([address(2)]),
        Err(NoncePoolError::UnknownAccount(address(2)))
    );
}

#[test]
fn should_remove_a_free_address() {
    let mut pool = pool_of([address(1), address(2)]);

    pool.remove_accounts([address(1)])
        .expect("removing a free address should succeed");

    assert_eq!(
        pool.addresses().copied().collect::<Vec<_>>(),
        vec![address(2)]
    );
}

fn pool_of(addresses: impl IntoIterator<Item = Address>) -> DurableNoncePool {
    DurableNoncePool::new(addresses).expect("the addresses are pairwise distinct")
}
