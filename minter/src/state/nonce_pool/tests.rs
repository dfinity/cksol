use crate::{
    state::nonce_pool::{DurableNoncePool, NoncePoolError},
    test_fixtures::{address, durable_nonce},
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

#[test]
fn should_list_every_account_of_a_new_pool_as_free() {
    let pool = pool_of([address(1), address(2)]);

    assert_eq!(free_accounts(&pool), vec![address(1), address(2)]);
}

#[test]
fn should_not_list_a_bound_account_as_free() {
    let mut pool = pool_of([address(1), address(2)]);

    pool.bind(&address(1), durable_nonce(1));

    assert_eq!(free_accounts(&pool), vec![address(2)]);
}

#[test]
fn should_list_a_freed_account_as_free_again() {
    let mut pool = pool_of([address(1)]);
    pool.bind(&address(1), durable_nonce(1));
    assert_eq!(free_accounts(&pool), vec![]);

    pool.free(&address(1));

    assert_eq!(free_accounts(&pool), vec![address(1)]);
}

#[test]
#[should_panic(expected = "already bound")]
fn should_panic_when_binding_a_bound_account() {
    let mut pool = pool_of([address(1)]);
    pool.bind(&address(1), durable_nonce(1));

    pool.bind(&address(1), durable_nonce(2));
}

#[test]
#[should_panic(expected = "already seen")]
fn should_panic_when_binding_a_seen_nonce_value() {
    let mut pool = pool_of([address(1)]);
    pool.bind(&address(1), durable_nonce(1));
    pool.free(&address(1));

    pool.bind(&address(1), durable_nonce(1));
}

#[test]
fn should_bind_a_freed_account_to_a_new_nonce_value() {
    let mut pool = pool_of([address(1)]);
    pool.bind(&address(1), durable_nonce(1));
    pool.free(&address(1));

    pool.bind(&address(1), durable_nonce(2));
}

#[test]
#[should_panic(expected = "not bound")]
fn should_panic_when_freeing_an_unbound_account() {
    let mut pool = pool_of([address(1)]);

    pool.free(&address(1));
}

#[test]
fn should_remember_the_nonce_values_of_past_bindings() {
    let mut pool = pool_of([address(1)]);
    pool.bind(&address(1), durable_nonce(1));
    pool.free(&address(1));
    pool.bind(&address(1), durable_nonce(2));

    assert!(pool.has_seen(&address(1), &durable_nonce(1)));
    assert!(pool.has_seen(&address(1), &durable_nonce(2)));
    assert!(!pool.has_seen(&address(1), &durable_nonce(3)));
}

#[test]
fn should_count_no_free_accounts_in_an_empty_pool() {
    assert_eq!(DurableNoncePool::default().num_free_accounts(), 0);
}

#[test]
fn should_count_only_the_free_accounts() {
    let mut pool = pool_of([address(1), address(2), address(3)]);
    assert_eq!(pool.num_free_accounts(), 3);

    pool.bind(&address(1), durable_nonce(1));
    pool.bind(&address(2), durable_nonce(2));

    assert_eq!(pool.num_free_accounts(), 1);
}

fn free_accounts(pool: &DurableNoncePool) -> Vec<Address> {
    pool.free_accounts().copied().collect()
}

fn pool_of(addresses: impl IntoIterator<Item = Address>) -> DurableNoncePool {
    DurableNoncePool::new(addresses).expect("the addresses are pairwise distinct")
}
