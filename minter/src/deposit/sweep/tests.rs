use crate::{
    deposit::sweep::{deposit_sol, deposit_status},
    test_fixtures::deposit::{DEPOSITOR_ACCOUNT, DEPOSITOR_PRINCIPAL},
};
use cksol_types::DepositSolStatus;
use icrc_ledger_types::icrc1::account::Account;

mod deposit_sol_tests {
    use super::{Account, DEPOSITOR_ACCOUNT, DEPOSITOR_PRINCIPAL, DepositSolStatus, deposit_sol};

    #[test]
    fn should_queue_deposit_with_nothing_to_sweep() {
        let result = deposit_sol(DEPOSITOR_ACCOUNT);

        assert_eq!(
            result,
            Ok(DepositSolStatus::Queued {
                account: DEPOSITOR_ACCOUNT,
                sweepable_amount: 0,
            })
        );
    }

    #[test]
    fn should_keep_subaccount_of_queued_deposit() {
        let account = Account {
            owner: DEPOSITOR_PRINCIPAL,
            subaccount: Some([7; 32]),
        };

        let result = deposit_sol(account);

        assert_eq!(
            result,
            Ok(DepositSolStatus::Queued {
                account,
                sweepable_amount: 0,
            })
        );
    }
}

mod deposit_status_tests {
    use super::{DEPOSITOR_ACCOUNT, deposit_sol, deposit_status};

    #[test]
    fn should_report_status_returned_by_deposit_sol() {
        let queued = deposit_sol(DEPOSITOR_ACCOUNT).expect("deposit_sol should succeed");

        let status = deposit_status(DEPOSITOR_ACCOUNT);

        assert_eq!(status, Some(queued));
    }
}
