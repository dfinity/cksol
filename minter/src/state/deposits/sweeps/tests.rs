use super::{Sweep, SweepRecoveryError, Sweeps, Transfer};
use crate::{
    constants::FEE_PER_SIGNATURE,
    state::event::VersionedMessage,
    test_fixtures::{
        MINTER_ADDRESS, account, planned_sweep, queued_deposit, queued_deposit_of, signature,
        sweep_message,
    },
};
use cksol_types::DepositSolId;
use sol_rpc_types::Lamport;

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

mod message {
    use super::{FEE_PER_SIGNATURE, Lamport, MINTER_ADDRESS, account, sweep_of};
    use crate::test_fixtures::deposit_address;
    use solana_hash::Hash;
    use solana_message::{Message, MessageHeader, compiled_instruction::CompiledInstruction};

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

    /// Historical sweep events replay only if a plan always compiles to the same message,
    /// so a failure here means a dependency bump broke replay — not that the expected
    /// message needs updating.
    #[test]
    fn should_build_the_message_recorded_for_the_plan() {
        let sweep = sweep_of(SWEEPABLE_AMOUNTS);

        let message = sweep.sweep_message(Hash::default());

        assert_eq!(
            message,
            Message {
                header: MessageHeader {
                    num_required_signatures: 3,
                    num_readonly_signed_accounts: 0,
                    num_readonly_unsigned_accounts: 1,
                },
                account_keys: vec![
                    deposit_address(account(1)),
                    deposit_address(account(3)),
                    deposit_address(account(2)),
                    MINTER_ADDRESS,
                    solana_system_interface::program::ID,
                ],
                recent_blockhash: Hash::default(),
                instructions: vec![
                    transfer_instruction(0, 30_000_000 - 3 * FEE_PER_SIGNATURE),
                    transfer_instruction(2, 20_000_000),
                    transfer_instruction(1, 10_000_000),
                ],
            }
        );
    }

    fn transfer_instruction(from_index: u8, amount: Lamport) -> CompiledInstruction {
        const TRANSFER_DISCRIMINANT: u32 = 2;
        const SYSTEM_PROGRAM_INDEX: u8 = 4;
        const MINTER_ADDRESS_INDEX: u8 = 3;
        let mut data = TRANSFER_DISCRIMINANT.to_le_bytes().to_vec();
        data.extend_from_slice(&amount.to_le_bytes());
        CompiledInstruction {
            program_id_index: SYSTEM_PROGRAM_INDEX,
            accounts: vec![from_index, MINTER_ADDRESS_INDEX],
            data,
        }
    }
}

mod mints {
    use super::{FEE_PER_SIGNATURE, Lamport, sweep_of};
    use crate::state::event::CreditedDeposit;

    #[test]
    fn should_share_the_fee_between_the_deposits_of_the_sweep() {
        let cases = [
            (
                "the fee is shared evenly",
                sweep_of([30_000_000, 20_000_000, 10_000_000]),
                vec![
                    (0, 30_000_000 - FEE_PER_SIGNATURE),
                    (1, 20_000_000 - FEE_PER_SIGNATURE),
                    (2, 10_000_000 - FEE_PER_SIGNATURE),
                ],
            ),
            (
                "a single deposit pays the whole fee",
                sweep_of([10_000_000]),
                vec![(0, 10_000_000 - FEE_PER_SIGNATURE)],
            ),
        ];
        for (name, sweep, expected_mints) in cases {
            let mints = sweep.mints();

            assert_eq!(
                mints
                    .iter()
                    .map(|mint| (mint.deposit_id, mint.amount_to_mint))
                    .collect::<Vec<_>>(),
                expected_mints,
                "{name}"
            );
            assert!(
                mints
                    .iter()
                    .map(|mint: &CreditedDeposit| mint.amount_to_mint)
                    .sum::<Lamport>()
                    <= sweep.expected_received(),
                "{name}"
            );
        }
    }
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

mod recover {
    use super::{Sweep, SweepRecoveryError, VersionedMessage, queued_deposit, sweep_message};
    use crate::test_fixtures::arb::{arb_address, arb_hash, arb_sweep_deposits};
    use proptest::{prop_assert_eq, proptest};

    proptest! {
        #[test]
        fn should_recover_the_planned_sweep_from_its_message(
            deposits in arb_sweep_deposits(),
            minter_address in arb_address(),
            blockhash in arb_hash(),
        ) {
            let planned = Sweep::plan(deposits.clone(), minter_address);
            let submitted = VersionedMessage::Legacy(planned.sweep_message(blockhash));

            let recovered = Sweep::recover(deposits, &submitted);

            prop_assert_eq!(recovered, Ok(planned));
        }
    }

    #[test]
    fn should_fail_when_the_plan_does_not_build_the_submitted_message() {
        let deposits = [(0, queued_deposit(0)), (1, queued_deposit(1))];
        let planned = sweep_message(deposits);
        type Mutation = fn(&mut solana_message::Message);
        let mutations: [(&str, Mutation); 3] = [
            ("another fee payer", |message| {
                message.account_keys.swap(0, 1)
            }),
            ("another amount", |message| {
                message.instructions[0].data[4] ^= 1
            }),
            ("a transfer less", |message| {
                message.instructions.pop();
            }),
        ];

        for (name, mutate) in mutations {
            let VersionedMessage::Legacy(mut message) = planned.clone();
            mutate(&mut message);
            let submitted = VersionedMessage::Legacy(message);

            let sweep = Sweep::recover(deposits, &submitted);

            assert_eq!(
                sweep,
                Err(SweepRecoveryError::UnexpectedMessage {
                    planned: Box::new(planned.clone()),
                    submitted: Box::new(submitted),
                }),
                "{name}"
            );
        }
    }

    #[test]
    fn should_fail_without_a_transfer_to_read_the_destination_from() {
        let deposit = queued_deposit(0);
        let message = solana_message::Message::new(&[], Some(&deposit.address));

        let sweep = Sweep::recover([(0, deposit)], &VersionedMessage::Legacy(message));

        assert_eq!(sweep, Err(SweepRecoveryError::MissingDestination));
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
#[should_panic(expected = "without deposits")]
fn should_panic_when_creating_a_sweep_without_deposits() {
    planned_sweep([]);
}

#[test]
#[should_panic(expected = "with deposit 0 twice")]
fn should_panic_when_creating_a_sweep_with_a_duplicated_deposit() {
    planned_sweep([(0, queued_deposit(0)), (0, queued_deposit(0))]);
}
