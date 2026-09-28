use super::{Deposits, QueuedDeposit, SweptDeposit};
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
        queue_three_deposits, queued, signature, sweep_deposits_two_and_zero, swept,
    };

    #[test]
    fn should_move_queued_deposits_to_swept_and_return_their_amounts() {
        let mut deposits = queue_three_deposits();
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);

        let amounts: Vec<_> = [2, 0]
            .into_iter()
            .map(|deposit_id| deposits.sweep(deposit_id, &sweep_signature))
            .collect();

        assert_eq!(amounts, vec![300, 100]);
        assert_eq!(
            deposits.swept(),
            &BTreeMap::from([
                (0, swept(0, sweep_signature)),
                (2, swept(2, sweep_signature))
            ])
        );
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
        queue_three_deposits().sweep(3, &signature(SWEEP_SIGNATURE_INDEX));
    }

    #[test]
    #[should_panic(expected = "Attempted to sweep unknown or already swept deposit 0")]
    fn should_panic_when_sweeping_already_swept_deposit() {
        let (mut deposits, _) = sweep_deposits_two_and_zero();

        deposits.sweep(0, &signature(SWEEP_SIGNATURE_INDEX + 1));
    }
}

mod resubmit_sweep {
    use super::{
        BTreeMap, DepositSolStatus, SWEEP_SIGNATURE_INDEX, assert_in_flight_ids_unchanged,
        signature, sweep_deposits_two_and_zero, swept,
    };

    #[test]
    fn should_report_the_new_signature_for_the_deposits_of_the_resubmitted_sweep() {
        let (mut deposits, expired_sweep) = sweep_deposits_two_and_zero();
        let unrelated_sweep = signature(SWEEP_SIGNATURE_INDEX + 1);
        deposits.sweep(1, &unrelated_sweep);
        let resubmitted_sweep = signature(SWEEP_SIGNATURE_INDEX + 2);

        deposits.resubmit_sweep(&expired_sweep, &resubmitted_sweep);

        assert_eq!(
            deposits.swept(),
            &BTreeMap::from([
                (0, swept(0, resubmitted_sweep)),
                (1, swept(1, unrelated_sweep)),
                (2, swept(2, resubmitted_sweep)),
            ])
        );
        assert_eq!(
            deposits.status(0),
            DepositSolStatus::Swept {
                signature: resubmitted_sweep.into()
            }
        );
        assert_in_flight_ids_unchanged(&deposits);
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
    for deposit_id in [2, 0] {
        deposits.sweep(deposit_id, &sweep_signature);
    }
    (deposits, sweep_signature)
}

fn queued(deposit_id: DepositSolId) -> QueuedDeposit {
    QueuedDeposit {
        account: account_of(deposit_id),
        sweepable_amount: 100 * (deposit_id + 1),
    }
}

fn swept(deposit_id: DepositSolId, sweep_signature: Signature) -> SweptDeposit {
    SweptDeposit {
        deposit: queued(deposit_id),
        signature: sweep_signature,
    }
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
