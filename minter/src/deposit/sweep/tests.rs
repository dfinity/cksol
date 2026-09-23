use crate::{
    deposit::sweep::{deposit_sol, deposit_status},
    test_fixtures::deposit::DEPOSITOR_ACCOUNT,
};
use candid::Principal;
use cksol_types::DepositSolStatus;
use icrc_ledger_types::icrc1::account::Account;

#[test]
fn should_queue_deposit_with_nothing_to_sweep() {
    let deposit_id = deposit_sol(DEPOSITOR_ACCOUNT).expect("deposit_sol should queue a sweep");

    let status = deposit_status(deposit_id);

    assert_eq!(
        status,
        Some(DepositSolStatus::Queued {
            sweepable_amount: 0
        })
    );
}

#[test]
#[should_panic(expected = "the owner must be non-anonymous")]
fn should_reject_anonymous_owner() {
    let anonymous_account = Account {
        owner: Principal::anonymous(),
        subaccount: None,
    };

    let _ = deposit_sol(anonymous_account);
}
