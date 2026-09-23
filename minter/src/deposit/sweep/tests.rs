use crate::{
    deposit::sweep::{deposit_sol, deposit_status},
    test_fixtures::deposit::DEPOSITOR_ACCOUNT,
};
use cksol_types::DepositSolStatus;

mod deposit_sol_tests {
    use super::{DEPOSITOR_ACCOUNT, deposit_sol};

    #[test]
    fn should_queue_deposit() {
        let result = deposit_sol(DEPOSITOR_ACCOUNT);

        assert!(result.is_ok());
    }
}

mod deposit_status_tests {
    use super::{DEPOSITOR_ACCOUNT, DepositSolStatus, deposit_sol, deposit_status};

    #[test]
    fn should_report_queued_deposit_with_nothing_to_sweep() {
        let deposit_id = deposit_sol(DEPOSITOR_ACCOUNT).expect("deposit_sol should queue a sweep");

        let status = deposit_status(deposit_id);

        assert_eq!(
            status,
            Some(DepositSolStatus::Queued {
                sweepable_amount: 0
            })
        );
    }
}
