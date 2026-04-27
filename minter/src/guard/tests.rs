use crate::{
    constants::MAX_CONCURRENT_HTTP_OUTCALLS,
    guard::{
        GuardError, HttpOutcallGuard, HttpOutcallGuardError, MAX_CONCURRENT, TimerGuard,
        TimerGuardError, process_deposit_guard, too_many_http_outcalls,
    },
    state::{TaskType, read_state},
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

mod guard {
    use super::*;

    #[test]
    fn should_prevent_concurrent_access_to_same_account() {
        init_state();

        // Effectively the same Account
        let account1 = account(0, None);
        let account2 = account(0, Some(0));
        {
            let _guard = process_deposit_guard(account1).unwrap();
            let res = process_deposit_guard(account2).err();
            assert_eq!(res, Some(GuardError::AlreadyProcessing));
        }
        let _guard = process_deposit_guard(account1).unwrap();
    }

    #[test]
    fn should_allow_access_after_guard_has_been_dropped() {
        init_state();

        let account = account(0, None);
        {
            let _guard = process_deposit_guard(account).unwrap();
        }
        let _guard = process_deposit_guard(account).unwrap();
    }

    #[test]
    fn should_prevent_more_than_max_concurrent_access() {
        init_state();

        let guards: Vec<_> = (0..MAX_CONCURRENT / 2)
            .map(|id| {
                process_deposit_guard(account(0, Some(id as u8))).unwrap_or_else(|e| {
                    panic!("Could not create guard for subaccount {id}: {e:#?}")
                })
            })
            .chain((MAX_CONCURRENT / 2..MAX_CONCURRENT).map(|id| {
                process_deposit_guard(account(id as u64, None))
                    .unwrap_or_else(|e| panic!("Could not create guard for principal {id}: {e:#?}"))
            }))
            .collect();
        assert_eq!(guards.len(), MAX_CONCURRENT);
        let account = account(MAX_CONCURRENT as u64 + 1, None);
        let res = process_deposit_guard(account).err();
        assert_eq!(res, Some(GuardError::TooManyConcurrentRequests));
    }
}

mod timer_guard {
    use super::*;

    #[test]
    fn should_create_guard_successfully() {
        init_state();

        let guard = TimerGuard::new(TaskType::DepositConsolidation);
        assert!(guard.is_ok());
    }

    #[test]
    fn should_prevent_concurrent_access_to_same_task() {
        init_state();

        let _guard = TimerGuard::new(TaskType::DepositConsolidation).unwrap();
        let result = TimerGuard::new(TaskType::DepositConsolidation);

        assert_eq!(result, Err(TimerGuardError::AlreadyProcessing));
    }

    #[test]
    fn should_allow_access_after_guard_has_been_dropped() {
        init_state();

        {
            let _guard = TimerGuard::new(TaskType::DepositConsolidation).unwrap();
        }

        let guard = TimerGuard::new(TaskType::DepositConsolidation);
        assert!(guard.is_ok());
    }

    #[test]
    fn should_allow_concurrent_access_to_different_tasks() {
        init_state();

        let _guard1 = TimerGuard::new(TaskType::DepositConsolidation).unwrap();
        let guard2 = TimerGuard::new(TaskType::Mint);

        assert!(guard2.is_ok());
    }
}

mod http_outcall_guard {
    use super::*;

    #[test]
    fn should_acquire_and_release_guard() {
        init_state();

        assert_eq!(read_state(|s| s.active_http_outcalls()), 0);
        {
            let _guard = HttpOutcallGuard::new().unwrap();
            assert_eq!(read_state(|s| s.active_http_outcalls()), 1);
        }
        assert_eq!(read_state(|s| s.active_http_outcalls()), 0);
    }

    #[test]
    fn should_allow_up_to_max_concurrent_guards() {
        init_state();

        let guards: Vec<_> = (0..MAX_CONCURRENT_HTTP_OUTCALLS)
            .map(|_| HttpOutcallGuard::new().expect("should succeed below limit"))
            .collect();
        assert_eq!(
            read_state(|s| s.active_http_outcalls()),
            MAX_CONCURRENT_HTTP_OUTCALLS
        );
        assert!(too_many_http_outcalls());
        drop(guards);
        assert_eq!(read_state(|s| s.active_http_outcalls()), 0);
        assert!(!too_many_http_outcalls());
    }

    #[test]
    fn should_reject_when_limit_reached() {
        init_state();

        let _guards: Vec<_> = (0..MAX_CONCURRENT_HTTP_OUTCALLS)
            .map(|_| HttpOutcallGuard::new().expect("should succeed below limit"))
            .collect();

        let result = HttpOutcallGuard::new();
        assert_eq!(result.err(), Some(HttpOutcallGuardError::TooManyOutcalls));
    }

    #[test]
    fn should_allow_new_guard_after_one_is_dropped() {
        init_state();

        let guards: Vec<_> = (0..MAX_CONCURRENT_HTTP_OUTCALLS)
            .map(|_| HttpOutcallGuard::new().expect("should succeed below limit"))
            .collect();

        // Drop one
        drop(guards);

        // Should be able to acquire a new guard
        let result = HttpOutcallGuard::new();
        assert!(result.is_ok());
    }

    #[test]
    fn should_track_multiple_concurrent_guards_independently() {
        init_state();

        let guard1 = HttpOutcallGuard::new().unwrap();
        let guard2 = HttpOutcallGuard::new().unwrap();
        assert_eq!(read_state(|s| s.active_http_outcalls()), 2);

        drop(guard1);
        assert_eq!(read_state(|s| s.active_http_outcalls()), 1);

        drop(guard2);
        assert_eq!(read_state(|s| s.active_http_outcalls()), 0);
    }
}
