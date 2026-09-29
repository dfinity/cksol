use super::{Sweep, Sweeps};
use crate::{state::QueuedDeposit, test_fixtures::account, test_fixtures::signature};
use cksol_types::DepositSolId;

#[test]
fn should_find_deposits_by_sweep_signature_and_by_deposit_id() {
    let mut sweeps = Sweeps::default();
    let first_sweep = signature(1);
    let second_sweep = signature(2);
    sweeps.insert(first_sweep, sweep_of([2, 0]));
    sweeps.insert(second_sweep, sweep_of([1]));

    assert_eq!(sweeps.get(&first_sweep), Some(&sweep_of([0, 2])));
    assert_eq!(sweeps.get(&signature(3)), None);
    assert_eq!(sweeps.deposit(0), Some((&first_sweep, &queued(0))));
    assert_eq!(sweeps.deposit(1), Some((&second_sweep, &queued(1))));
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
    sweeps.insert(removed_sweep, sweep_of([2, 0]));
    sweeps.insert(kept_sweep, sweep_of([1]));

    assert_eq!(sweeps.remove(&removed_sweep), Some(sweep_of([0, 2])));

    assert_eq!(sweeps.remove(&removed_sweep), None);
    assert_eq!(sweeps.deposit(0), None);
    assert_eq!(sweeps.deposit(1), Some((&kept_sweep, &queued(1))));
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
    sweeps.insert(sweep_signature, sweep_of([0]));

    sweeps.insert(sweep_signature, sweep_of([1]));
}

#[test]
fn should_sum_the_swept_amount_of_a_sweep() {
    assert_eq!(sweep_of([2, 0]).swept_amount(), 300 + 100);
    assert_eq!(sweep_of([1]).swept_amount(), 200);
}

#[test]
#[should_panic(expected = "without deposits")]
fn should_panic_when_creating_a_sweep_without_deposits() {
    sweep_of([]);
}

fn sweep_of<const N: usize>(deposit_ids: [DepositSolId; N]) -> Sweep {
    Sweep::new(
        deposit_ids
            .into_iter()
            .map(|deposit_id| (deposit_id, queued(deposit_id)))
            .collect(),
    )
}

fn queued(deposit_id: DepositSolId) -> QueuedDeposit {
    QueuedDeposit {
        account: account(deposit_id as usize + 1),
        sweepable_amount: 100 * (deposit_id + 1),
    }
}
