use crate::{
    constants::FEE_PER_SIGNATURE,
    state::Sweep,
    test_fixtures::{MINTER_ADDRESS, account, planned_sweep, queued_deposit_of},
};
use cksol_types::DepositSolId;
use sol_rpc_types::Lamport;
use solana_hash::Hash;

const SWEEPABLE_AMOUNTS: [Lamport; 3] = [30_000_000, 20_000_000, 10_000_000];

#[test]
fn should_transfer_from_every_deposit_address_to_the_minter_address() {
    let sweep = sweep_of(SWEEPABLE_AMOUNTS);
    let transfers = sweep.transfers();

    let message = sweep.sweep_message(Hash::default());

    assert_eq!(message.header.num_required_signatures, 3);
    assert_eq!(message.account_keys[0], transfers[0].from);
    let mut other_signers = vec![transfers[1].from, transfers[2].from];
    other_signers.sort();
    assert_eq!(message.account_keys[1..3], other_signers);
    assert_eq!(
        message.account_keys[3..],
        [MINTER_ADDRESS, solana_system_interface::program::ID]
    );
    assert_eq!(message.instructions.len(), 3);
    assert_eq!(transfers[0].amount, 30_000_000 - 3 * FEE_PER_SIGNATURE);
    assert_eq!(transfers[1].amount, 20_000_000);
    assert_eq!(transfers[2].amount, 10_000_000);
}

/// The sweep of the given sweepable amounts from the accounts `1..`, deposit ids from `0`.
fn sweep_of<const N: usize>(sweepable_amounts: [Lamport; N]) -> Sweep {
    planned_sweep(
        sweepable_amounts
            .into_iter()
            .enumerate()
            .map(|(index, sweepable_amount)| {
                (
                    index as DepositSolId,
                    queued_deposit_of(account(index + 1), sweepable_amount),
                )
            }),
    )
}
