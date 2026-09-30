use super::{DepositBalance, Deposits, Sweep};
use crate::{
    constants::RENT_EXEMPTION_THRESHOLD,
    test_fixtures::{queued_deposit, queued_deposit_of, signature},
};
use cksol_types::DepositSolStatus;
use sol_rpc_types::Lamport;
use std::collections::BTreeMap;

const SWEEP_SIGNATURE_INDEX: usize = 0xAA;

mod deposit_balance {
    use super::{DepositBalance, Lamport, RENT_EXEMPTION_THRESHOLD};

    #[test]
    fn should_reject_a_balance_below_the_rent_exemption_threshold() {
        for balance in [0, RENT_EXEMPTION_THRESHOLD - 1] {
            assert_eq!(DepositBalance::new(balance), None, "balance {balance}");
        }
    }

    #[test]
    fn should_sweep_the_balance_above_the_rent_exemption_threshold() {
        for (balance, expected) in [
            (RENT_EXEMPTION_THRESHOLD, 0),
            (RENT_EXEMPTION_THRESHOLD + 1, 1),
            (Lamport::MAX, Lamport::MAX - RENT_EXEMPTION_THRESHOLD),
        ] {
            let deposit_balance = DepositBalance::new(balance).expect("rent-exempt balance");

            assert_eq!(deposit_balance.get(), balance, "balance {balance}");
            assert_eq!(
                deposit_balance.sweepable_amount(),
                expected,
                "balance {balance}"
            );
        }
    }
}

mod queue {
    use super::{BTreeMap, DepositSolStatus, Deposits, queued_deposit, queued_deposit_of};

    #[test]
    fn should_assign_sequential_ids_and_keep_accounts_in_flight() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));

        assert_eq!(deposits.next_id(), 2);
        assert_eq!(
            deposits.queued(),
            &BTreeMap::from([(0, queued_deposit(0)), (1, queued_deposit(1))])
        );
        assert_eq!(deposits.in_flight_id(&queued_deposit(0).account), Some(0));
        assert_eq!(deposits.in_flight_id(&queued_deposit(1).account), Some(1));
        assert_eq!(deposits.in_flight_id(&queued_deposit(2).account), None);
    }

    #[test]
    fn should_report_the_status_of_queued_and_unknown_deposits() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));

        assert_eq!(
            deposits.status(0),
            DepositSolStatus::Queued {
                sweepable_amount: queued_deposit(0).sweepable_amount()
            }
        );
        assert_eq!(deposits.status(1), DepositSolStatus::NotFound);
    }

    #[test]
    #[should_panic(expected = "out of sequence")]
    fn should_panic_if_deposit_id_out_of_sequence() {
        Deposits::default().queue(1, queued_deposit(1));
    }

    #[test]
    #[should_panic(expected = "already has one in flight")]
    fn should_panic_if_account_already_in_flight() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));

        deposits.queue(1, queued_deposit_of(queued_deposit(0).account, 200));
    }
}

mod sweep {
    use super::{
        BTreeMap, DepositSolStatus, Deposits, SWEEP_SIGNATURE_INDEX, Sweep, queued_deposit,
        signature,
    };

    #[test]
    fn should_move_queued_deposits_to_a_sweep_and_return_the_swept_amount() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));
        deposits.queue(2, queued_deposit(2));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);

        let swept_amount = deposits.sweep(&[2, 0], &sweep_signature);

        assert_eq!(
            swept_amount,
            queued_deposit(2).sweepable_amount() + queued_deposit(0).sweepable_amount()
        );
        assert_eq!(
            deposits.swept().get(&sweep_signature),
            Some(&Sweep::new([
                (0, queued_deposit(0)),
                (2, queued_deposit(2))
            ]))
        );
        assert_eq!(deposits.swept().len(), 1);
        assert_eq!(deposits.queued(), &BTreeMap::from([(1, queued_deposit(1))]));
        for deposit_id in 0..3 {
            assert_eq!(
                deposits.in_flight_id(&queued_deposit(deposit_id).account),
                Some(deposit_id)
            );
        }
        assert_eq!(
            deposits.status(0),
            DepositSolStatus::Swept {
                signature: sweep_signature.into()
            }
        );
        assert_eq!(
            deposits.status(1),
            DepositSolStatus::Queued {
                sweepable_amount: queued_deposit(1).sweepable_amount()
            }
        );
    }

    #[test]
    #[should_panic(expected = "Attempted to sweep unknown or already swept deposit 3")]
    fn should_panic_when_sweeping_unknown_deposit() {
        Deposits::default().sweep(&[3], &signature(SWEEP_SIGNATURE_INDEX));
    }

    #[test]
    #[should_panic(expected = "Attempted to sweep unknown or already swept deposit 0")]
    fn should_panic_when_sweeping_already_swept_deposit() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.sweep(&[0], &signature(SWEEP_SIGNATURE_INDEX));

        deposits.sweep(&[0], &signature(SWEEP_SIGNATURE_INDEX + 1));
    }

    #[test]
    #[should_panic(expected = "Attempted to sweep no deposits")]
    fn should_panic_when_sweeping_no_deposits() {
        Deposits::default().sweep(&[], &signature(SWEEP_SIGNATURE_INDEX));
    }

    #[test]
    #[should_panic(expected = "Attempted to record sweep")]
    fn should_panic_when_reusing_a_sweep_signature() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(&[0], &sweep_signature);

        deposits.sweep(&[1], &sweep_signature);
    }
}

mod resubmit_sweep {
    use super::{
        DepositSolStatus, Deposits, SWEEP_SIGNATURE_INDEX, Sweep, queued_deposit, signature,
    };

    #[test]
    fn should_move_the_deposits_of_the_resubmitted_sweep_to_the_new_signature() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));
        deposits.queue(2, queued_deposit(2));
        let expired_sweep = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(&[2, 0], &expired_sweep);
        let unrelated_sweep = signature(SWEEP_SIGNATURE_INDEX + 1);
        deposits.sweep(&[1], &unrelated_sweep);
        let resubmitted_sweep = signature(SWEEP_SIGNATURE_INDEX + 2);

        deposits.resubmit_sweep(&expired_sweep, &resubmitted_sweep);

        assert_eq!(deposits.swept().get(&expired_sweep), None);
        assert_eq!(
            deposits.swept().get(&resubmitted_sweep),
            Some(&Sweep::new([
                (0, queued_deposit(0)),
                (2, queued_deposit(2))
            ]))
        );
        assert_eq!(
            deposits.swept().get(&unrelated_sweep),
            Some(&Sweep::new([(1, queued_deposit(1))]))
        );
        assert_eq!(
            deposits.status(0),
            DepositSolStatus::Swept {
                signature: resubmitted_sweep.into()
            }
        );
        for deposit_id in 0..3 {
            assert_eq!(
                deposits.in_flight_id(&queued_deposit(deposit_id).account),
                Some(deposit_id)
            );
        }
    }

    #[test]
    fn should_ignore_a_resubmitted_transaction_that_is_not_a_sweep() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(&[0], &sweep_signature);

        deposits.resubmit_sweep(
            &signature(SWEEP_SIGNATURE_INDEX + 1),
            &signature(SWEEP_SIGNATURE_INDEX + 2),
        );

        assert_eq!(deposits.swept().len(), 1);
        assert_eq!(
            deposits.swept().get(&sweep_signature),
            Some(&Sweep::new([(0, queued_deposit(0))]))
        );
    }
}
