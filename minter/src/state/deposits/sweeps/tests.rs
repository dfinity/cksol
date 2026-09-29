use super::{Sweep, Sweeps};
use crate::{
    state::QueuedDeposit,
    test_fixtures::{account, queued_deposit, signature},
};

#[test]
fn should_find_deposits_by_sweep_signature_and_by_deposit_id() {
    let mut sweeps = Sweeps::default();
    let first_sweep = signature(1);
    let second_sweep = signature(2);
    sweeps.insert(
        first_sweep,
        Sweep::new([(0, queued_deposit(0)), (2, queued_deposit(2))]),
    );
    sweeps.insert(second_sweep, Sweep::new([(1, queued_deposit(1))]));

    assert_eq!(
        sweeps.get(&first_sweep),
        Some(&Sweep::new([
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
        Sweep::new([(0, queued_deposit(0)), (2, queued_deposit(2))]),
    );
    sweeps.insert(kept_sweep, Sweep::new([(1, queued_deposit(1))]));

    assert_eq!(
        sweeps.remove(&removed_sweep),
        Some(Sweep::new([(0, queued_deposit(0)), (2, queued_deposit(2))]))
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
    sweeps.insert(sweep_signature, Sweep::new([(0, queued_deposit(0))]));

    sweeps.insert(sweep_signature, Sweep::new([(1, queued_deposit(1))]));
}

#[test]
fn should_sum_the_sweepable_amounts_of_the_deposits() {
    let sweep = Sweep::new([
        (
            0,
            QueuedDeposit {
                account: account(1),
                sweepable_amount: 1_000,
            },
        ),
        (
            1,
            QueuedDeposit {
                account: account(2),
                sweepable_amount: 250,
            },
        ),
    ]);

    assert_eq!(sweep.swept_amount(), 1_250);
}

#[test]
#[should_panic(expected = "without deposits")]
fn should_panic_when_creating_a_sweep_without_deposits() {
    Sweep::new([]);
}
