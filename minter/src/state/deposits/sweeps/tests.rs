use super::{Sweeps, Transfer};
use crate::{
    constants::FEE_PER_SIGNATURE,
    test_fixtures::{
        MINTER_ADDRESS, account, planned_sweep, queued_deposit, queued_deposit_of, signature,
    },
};

#[test]
fn should_find_deposits_by_sweep_signature_and_by_deposit_id() {
    let mut sweeps = Sweeps::default();
    let first_sweep = signature(1);
    let second_sweep = signature(2);
    sweeps.insert(
        first_sweep,
        planned_sweep([(0, queued_deposit(0)), (2, queued_deposit(2))]),
    );
    sweeps.insert(second_sweep, planned_sweep([(1, queued_deposit(1))]));

    assert_eq!(
        sweeps.get(&first_sweep),
        Some(&planned_sweep([
            (0, queued_deposit(0)),
            (2, queued_deposit(2))
        ]))
    );
    assert_eq!(sweeps.get(&signature(3)), None);
    assert_eq!(sweeps.deposit(0), Some((&first_sweep, &queued_deposit(0))));
    assert_eq!(sweeps.deposit(1), Some((&second_sweep, &queued_deposit(1))));
    assert_eq!(sweeps.deposit(3), None);
    assert_eq!(
        sweeps.signatures().collect::<Vec<_>>(),
        vec![&first_sweep, &second_sweep]
    );
    assert_eq!(sweeps.len(), 2);
    assert_eq!(sweeps.deposit_count(), 3);
    assert!(!sweeps.is_empty());
}

#[test]
fn should_remove_a_sweep_by_signature() {
    let mut sweeps = Sweeps::default();
    let removed_sweep = signature(1);
    let kept_sweep = signature(2);
    sweeps.insert(
        removed_sweep,
        planned_sweep([(0, queued_deposit(0)), (2, queued_deposit(2))]),
    );
    sweeps.insert(kept_sweep, planned_sweep([(1, queued_deposit(1))]));

    assert_eq!(
        sweeps.remove(&removed_sweep),
        Some(planned_sweep([
            (0, queued_deposit(0)),
            (2, queued_deposit(2))
        ]))
    );

    assert_eq!(sweeps.remove(&removed_sweep), None);
    assert_eq!(sweeps.deposit(0), None);
    assert_eq!(sweeps.deposit(1), Some((&kept_sweep, &queued_deposit(1))));
    assert_eq!(sweeps.deposit_count(), 1);
}

#[test]
fn should_report_an_empty_collection() {
    let sweeps = Sweeps::default();

    assert!(sweeps.is_empty());
    assert_eq!(sweeps.len(), 0);
    assert_eq!(sweeps.deposit_count(), 0);
    assert_eq!(sweeps.signatures().count(), 0);
}

#[test]
#[should_panic(expected = "Attempted to record sweep")]
fn should_panic_when_inserting_a_sweep_with_a_known_signature() {
    let mut sweeps = Sweeps::default();
    let sweep_signature = signature(1);
    sweeps.insert(sweep_signature, planned_sweep([(0, queued_deposit(0))]));

    sweeps.insert(sweep_signature, planned_sweep([(1, queued_deposit(1))]));
}

mod plan {
    use super::{
        FEE_PER_SIGNATURE, MINTER_ADDRESS, Transfer, account, planned_sweep, queued_deposit_of,
    };

    #[test]
    fn should_charge_the_fee_to_the_largest_deposit_and_transfer_it_first() {
        let smallest = queued_deposit_of(account(1), 1_000_000);
        let largest = queued_deposit_of(account(2), 3_000_000);
        let middle = queued_deposit_of(account(3), 2_000_000);

        let sweep = planned_sweep([(0, smallest), (1, largest), (2, middle)]);

        assert_eq!(sweep.fee_payer(), 1);
        assert_eq!(sweep.fee(), 3 * FEE_PER_SIGNATURE);
        assert_eq!(sweep.minter_address(), MINTER_ADDRESS);
        assert_eq!(
            sweep.transfers(),
            vec![
                Transfer {
                    deposit_id: 1,
                    from: largest.address,
                    amount: 3_000_000 - 3 * FEE_PER_SIGNATURE,
                },
                Transfer {
                    deposit_id: 2,
                    from: middle.address,
                    amount: 2_000_000,
                },
                Transfer {
                    deposit_id: 0,
                    from: smallest.address,
                    amount: 1_000_000,
                },
            ]
        );
        assert_eq!(sweep.expected_received(), 6_000_000 - 3 * FEE_PER_SIGNATURE);
    }

    #[test]
    fn should_break_ties_by_the_lowest_deposit_id() {
        let first = queued_deposit_of(account(1), 1_000_000);
        let second = queued_deposit_of(account(2), 1_000_000);

        let sweep = planned_sweep([(3, second), (1, first)]);

        assert_eq!(sweep.fee_payer(), 1);
        assert_eq!(
            sweep
                .transfers()
                .iter()
                .map(|transfer| transfer.deposit_id)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
    }

    #[test]
    fn should_charge_the_whole_fee_to_a_single_deposit() {
        let sweep = planned_sweep([(7, queued_deposit_of(account(1), 1_000_000))]);

        assert_eq!(sweep.fee_payer(), 7);
        assert_eq!(sweep.fee(), FEE_PER_SIGNATURE);
        assert_eq!(sweep.expected_received(), 1_000_000 - FEE_PER_SIGNATURE);
    }
}

#[test]
#[should_panic(expected = "while another sweep holds it")]
fn should_panic_when_inserting_a_sweep_whose_deposit_another_sweep_holds() {
    let mut sweeps = Sweeps::default();
    sweeps.insert(signature(1), planned_sweep([(0, queued_deposit(0))]));

    sweeps.insert(
        signature(2),
        planned_sweep([(0, queued_deposit(0)), (1, queued_deposit(1))]),
    );
}

#[test]
fn should_sum_the_sweepable_amounts_of_the_deposits() {
    let sweep = planned_sweep([
        (0, queued_deposit_of(account(1), 1_000)),
        (1, queued_deposit_of(account(2), 250)),
    ]);

    assert_eq!(sweep.swept_amount(), 1_250);
}

#[test]
#[should_panic(expected = "without deposits")]
fn should_panic_when_creating_a_sweep_without_deposits() {
    planned_sweep([]);
}

#[test]
#[should_panic(expected = "with deposit 0 twice")]
fn should_panic_when_creating_a_sweep_with_a_duplicated_deposit() {
    planned_sweep([(0, queued_deposit(0)), (0, queued_deposit(0))]);
}
