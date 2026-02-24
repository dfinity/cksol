use crate::{
    guard::{GuardError, MAX_CONCURRENT, update_balance_guard},
    test_fixtures::init_state,
};
use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

fn principal(id: u64) -> Principal {
    Principal::try_from_slice(&id.to_le_bytes()).unwrap()
}

fn account(id: u64, sub: Option<u8>) -> Account {
    Account {
        owner: principal(id),
        subaccount: sub.map(|i| [i; 32]),
    }
}

#[test]
fn should_prevent_concurrent_access_to_same_account() {
    init_state();

    // Effectively the same Account
    let account1 = account(0, None);
    let account2 = account(0, Some(0));
    {
        let _guard = update_balance_guard(account1).unwrap();
        let res = update_balance_guard(account2).err();
        assert_eq!(res, Some(GuardError::AlreadyProcessing));
    }
    let _guard = update_balance_guard(account1).unwrap();
}

#[test]
fn should_allow_access_after_guard_has_been_dropped() {
    init_state();

    let account = account(0, None);
    {
        let _guard = update_balance_guard(account).unwrap();
    }
    let _guard = update_balance_guard(account).unwrap();
}

#[test]
fn should_prevent_more_than_max_concurrent_access() {
    init_state();

    let guards: Vec<_> = (0..MAX_CONCURRENT / 2)
        .map(|id| {
            update_balance_guard(account(0, Some(id as u8)))
                .unwrap_or_else(|e| panic!("Could not create guard for subaccount {id}: {e:#?}"))
        })
        .chain((MAX_CONCURRENT / 2..MAX_CONCURRENT).map(|id| {
            update_balance_guard(account(id as u64, None))
                .unwrap_or_else(|e| panic!("Could not create guard for principal {id}: {e:#?}"))
        }))
        .collect();
    assert_eq!(guards.len(), MAX_CONCURRENT);
    let account = account(MAX_CONCURRENT as u64 + 1, None);
    let res = update_balance_guard(account).err();
    assert_eq!(res, Some(GuardError::TooManyConcurrentRequests));
}
