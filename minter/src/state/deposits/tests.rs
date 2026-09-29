use super::{Deposits, QueuedDeposit, Sweep};
use crate::test_fixtures::{account, signature};
use cksol_types::{DepositSolId, DepositSolStatus};
use icrc_ledger_types::icrc1::account::Account;
use solana_signature::Signature;
use std::collections::BTreeMap;

const SWEEP_SIGNATURE_INDEX: usize = 0xAA;

mod queue {
    use super::{
        BTreeMap, DepositSolStatus, Deposits, QueuedDeposit, account,
        assert_in_flight_ids_unchanged, queue_three_deposits, queued,
    };

    #[test]
    fn should_assign_sequential_ids_and_keep_accounts_in_flight() {
        let deposits = queue_three_deposits();

        assert_eq!(deposits.next_id(), 3);
        assert_eq!(
            deposits.queued(),
            &BTreeMap::from([(0, queued(0)), (1, queued(1)), (2, queued(2))])
        );
        assert_in_flight_ids_unchanged(&deposits);
        assert_eq!(deposits.in_flight_id(&account(4)), None);
    }

    #[test]
    fn should_report_the_status_of_queued_and_unknown_deposits() {
        let deposits = queue_three_deposits();

        assert_eq!(
            deposits.status(1),
            DepositSolStatus::Queued {
                sweepable_amount: 200
            }
        );
        assert_eq!(deposits.status(3), DepositSolStatus::NotFound);
    }

    #[test]
    #[should_panic(expected = "out of sequence")]
    fn should_panic_if_deposit_id_out_of_sequence() {
        Deposits::default().queue(1, queued(1));
    }

    #[test]
    #[should_panic(expected = "already has one in flight")]
    fn should_panic_if_account_already_in_flight() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued(0));

        deposits.queue(
            1,
            QueuedDeposit {
                account: account(1),
                sweepable_amount: 200,
            },
        );
    }
}

mod sweep {
    use super::{
        BTreeMap, DepositSolStatus, SWEEP_SIGNATURE_INDEX, assert_in_flight_ids_unchanged,
        queue_three_deposits, queued, signature, sweep_deposits_two_and_zero, sweep_of,
    };

    #[test]
    fn should_move_queued_deposits_to_a_sweep_and_return_the_swept_amount() {
        let mut deposits = queue_three_deposits();
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);

        let swept_amount = deposits.sweep(&[2, 0], &sweep_signature);

        assert_eq!(swept_amount, 300 + 100);
        assert_eq!(
            deposits.swept().get(&sweep_signature),
            Some(&sweep_of([0, 2]))
        );
        assert_eq!(deposits.swept().len(), 1);
        assert_eq!(deposits.queued(), &BTreeMap::from([(1, queued(1))]));
        assert_in_flight_ids_unchanged(&deposits);
        assert_eq!(
            deposits.status(0),
            DepositSolStatus::Swept {
                signature: sweep_signature.into()
            }
        );
        assert_eq!(
            deposits.status(1),
            DepositSolStatus::Queued {
                sweepable_amount: 200
            }
        );
    }

    #[test]
    #[should_panic(expected = "Attempted to sweep unknown or already swept deposit 3")]
    fn should_panic_when_sweeping_unknown_deposit() {
        queue_three_deposits().sweep(&[3], &signature(SWEEP_SIGNATURE_INDEX));
    }

    #[test]
    #[should_panic(expected = "Attempted to sweep unknown or already swept deposit 0")]
    fn should_panic_when_sweeping_already_swept_deposit() {
        let (mut deposits, _) = sweep_deposits_two_and_zero();

        deposits.sweep(&[0], &signature(SWEEP_SIGNATURE_INDEX + 1));
    }

    #[test]
    #[should_panic(expected = "Attempted to sweep no deposits")]
    fn should_panic_when_sweeping_no_deposits() {
        queue_three_deposits().sweep(&[], &signature(SWEEP_SIGNATURE_INDEX));
    }

    #[test]
    #[should_panic(expected = "Attempted to record sweep")]
    fn should_panic_when_reusing_a_sweep_signature() {
        let (mut deposits, sweep_signature) = sweep_deposits_two_and_zero();

        deposits.sweep(&[1], &sweep_signature);
    }
}

mod resubmit_sweep {
    use super::{
        DepositSolStatus, SWEEP_SIGNATURE_INDEX, assert_in_flight_ids_unchanged, signature,
        sweep_deposits_two_and_zero, sweep_of,
    };

    #[test]
    fn should_move_the_deposits_of_the_resubmitted_sweep_to_the_new_signature() {
        let (mut deposits, expired_sweep) = sweep_deposits_two_and_zero();
        let unrelated_sweep = signature(SWEEP_SIGNATURE_INDEX + 1);
        deposits.sweep(&[1], &unrelated_sweep);
        let resubmitted_sweep = signature(SWEEP_SIGNATURE_INDEX + 2);

        deposits.resubmit_sweep(&expired_sweep, &resubmitted_sweep);

        assert_eq!(deposits.swept().get(&expired_sweep), None);
        assert_eq!(
            deposits.swept().get(&resubmitted_sweep),
            Some(&sweep_of([0, 2]))
        );
        assert_eq!(deposits.swept().get(&unrelated_sweep), Some(&sweep_of([1])));
        assert_eq!(
            deposits.status(0),
            DepositSolStatus::Swept {
                signature: resubmitted_sweep.into()
            }
        );
        assert_in_flight_ids_unchanged(&deposits);
    }

    #[test]
    fn should_ignore_a_resubmitted_transaction_that_is_not_a_sweep() {
        let (mut deposits, sweep_signature) = sweep_deposits_two_and_zero();

        deposits.resubmit_sweep(
            &signature(SWEEP_SIGNATURE_INDEX + 1),
            &signature(SWEEP_SIGNATURE_INDEX + 2),
        );

        assert_eq!(deposits.swept().len(), 1);
        assert_eq!(
            deposits.swept().get(&sweep_signature),
            Some(&sweep_of([0, 2]))
        );
    }
}

fn queue_three_deposits() -> Deposits {
    let mut deposits = Deposits::default();
    for deposit_id in 0..3 {
        deposits.queue(deposit_id, queued(deposit_id));
    }
    deposits
}

fn sweep_deposits_two_and_zero() -> (Deposits, Signature) {
    let mut deposits = queue_three_deposits();
    let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
    deposits.sweep(&[2, 0], &sweep_signature);
    (deposits, sweep_signature)
}

fn queued(deposit_id: DepositSolId) -> QueuedDeposit {
    QueuedDeposit {
        account: account_of(deposit_id),
        sweepable_amount: 100 * (deposit_id + 1),
    }
}

fn sweep_of<const N: usize>(deposit_ids: [DepositSolId; N]) -> Sweep {
    Sweep::new(
        deposit_ids
            .into_iter()
            .map(|deposit_id| (deposit_id, queued(deposit_id)))
            .collect(),
    )
}

fn account_of(deposit_id: DepositSolId) -> Account {
    account(deposit_id as usize + 1)
}

fn assert_in_flight_ids_unchanged(deposits: &Deposits) {
    for deposit_id in 0..3 {
        assert_eq!(
            deposits.in_flight_id(&account_of(deposit_id)),
            Some(deposit_id)
        );
    }
}
