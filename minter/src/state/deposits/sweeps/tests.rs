use super::{
    Sweep, SweepMismatch, SweepRecoveryError, SweepSettlementError, Sweeps, Transfer,
    UnreadableOutcome,
};
use crate::{
    constants::{FEE_PER_SIGNATURE, RENT_EXEMPTION_THRESHOLD},
    state::event::VersionedMessage,
    test_fixtures::{
        MINTER_ADDRESS, account, planned_sweep, queued_deposit, queued_deposit_of, signature,
        sweep_message,
        sweep_outcome::{MAIN_BALANCE, SweepOutcome},
    },
};
use cksol_types::DepositSolId;
use serde_json::json;
use sol_rpc_types::Lamport;
use solana_address::{Address, address};
use solana_hash::Hash;
use solana_transaction::TransactionError;
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction, UiTransactionError,
};

const SWEEPABLE_AMOUNTS: [Lamport; 3] = [30_000_000, 20_000_000, 10_000_000];
const PLANNED_FEE: Lamport = FEE_PER_SIGNATURE * SWEEPABLE_AMOUNTS.len() as u64;

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
    use super::{
        FEE_PER_SIGNATURE, Hash, Lamport, MINTER_ADDRESS, SWEEPABLE_AMOUNTS, account, sweep_of,
    };
    use crate::test_fixtures::deposit_address;
    use solana_message::{Message, MessageHeader, compiled_instruction::CompiledInstruction};

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

mod settle {
    use super::{
        Address, MAIN_BALANCE, MINTER_ADDRESS, PLANNED_FEE, RENT_EXEMPTION_THRESHOLD,
        SWEEPABLE_AMOUNTS, Sweep, SweepMismatch, SweepOutcome, SweepSettlementError,
        TransactionError, UiTransactionError, UnreadableOutcome, account, mutate_blob,
        planned_sweep, queued_deposit_of, sweep_of,
    };

    #[test]
    fn should_settle_an_outcome_matching_the_plan() {
        let sweep = sweep_of(SWEEPABLE_AMOUNTS);
        let second_deposit = sweep.transfers()[1].from;
        let cases = [
            ("the fee was as planned", SweepOutcome::of(&sweep)),
            (
                "a late transfer left extra on a deposit address",
                SweepOutcome::of(&sweep).with_late_transfer(second_deposit, 123_456),
            ),
            (
                "a lower fee left extra on the fee payer",
                SweepOutcome::of(&sweep).with_fee(PLANNED_FEE - 1_000),
            ),
        ];

        for (name, outcome) in cases {
            let settled = sweep
                .clone()
                .settle(&outcome.encode())
                .unwrap_or_else(|e| panic!("{name}: {e}"));

            assert_eq!(
                settled.amount_received(),
                sweep.expected_received(),
                "{name}"
            );
            assert_eq!(settled.into_mints(), sweep.mints(), "{name}");
        }
    }

    #[test]
    fn should_reject_an_outcome_that_cannot_be_read() {
        let sweep = sweep_of(SWEEPABLE_AMOUNTS);
        let mut incomplete_balances = SweepOutcome::of(&sweep).encode();
        incomplete_balances
            .transaction
            .meta
            .as_mut()
            .expect("meta")
            .post_balances
            .pop();
        let cases = [
            (
                "the transaction cannot be decoded",
                mutate_blob(SweepOutcome::of(&sweep).encode()),
                UnreadableOutcome::TransactionDecodingFailed,
            ),
            (
                "the meta field is missing",
                SweepOutcome::of(&sweep).encode_without_meta(),
                UnreadableOutcome::NoMetaField,
            ),
            (
                "the balances do not cover all account keys",
                incomplete_balances,
                UnreadableOutcome::IncompleteBalances,
            ),
        ];

        for (name, outcome, expected) in cases {
            assert_eq!(
                sweep.clone().settle(&outcome),
                Err(SweepSettlementError::Unreadable(expected)),
                "{name}"
            );
        }
    }

    #[test]
    fn should_reject_an_outcome_that_does_not_match_the_plan() {
        let sweep = sweep_of(SWEEPABLE_AMOUNTS);
        let transfers = sweep.transfers();
        let fee_payer = transfers[0].from;
        let second_deposit = transfers[1].from;
        let other_minter = Address::from([0x77; 32]);
        let other_sweep = planned_sweep(
            [(9, queued_deposit_of(account(9), 5_000_000))]
                .into_iter()
                .chain(sweep.deposits().iter().map(|(id, deposit)| (*id, *deposit))),
        );
        let cases = [
            (
                "the transaction failed",
                SweepOutcome::of(&sweep).with_error(UiTransactionError::from(
                    TransactionError::InsufficientFundsForFee,
                )),
                SweepMismatch::TransactionFailed {
                    error: TransactionError::InsufficientFundsForFee.to_string(),
                },
            ),
            (
                "the executed message is another sweep",
                SweepOutcome::of(&sweep)
                    .with_message(other_sweep.sweep_message(Default::default())),
                SweepMismatch::UnexpectedMessage,
            ),
            (
                "the executed message sweeps to another address",
                SweepOutcome::of(&sweep).with_message(
                    Sweep::plan(
                        sweep.deposits().iter().map(|(id, deposit)| (*id, *deposit)),
                        other_minter,
                    )
                    .sweep_message(Default::default()),
                ),
                SweepMismatch::UnexpectedMessage,
            ),
            (
                "the fee exceeds the planned fee",
                SweepOutcome::of(&sweep).with_fee(PLANNED_FEE + 1),
                SweepMismatch::UnexpectedFee {
                    expected: PLANNED_FEE,
                    actual: PLANNED_FEE + 1,
                },
            ),
            (
                "a deposit address moved by the wrong amount",
                SweepOutcome::of(&sweep).with_balances(
                    second_deposit,
                    RENT_EXEMPTION_THRESHOLD + SWEEPABLE_AMOUNTS[1],
                    RENT_EXEMPTION_THRESHOLD + 1,
                ),
                SweepMismatch::UnexpectedBalanceChange {
                    address: second_deposit,
                    pre: RENT_EXEMPTION_THRESHOLD + SWEEPABLE_AMOUNTS[1],
                    post: RENT_EXEMPTION_THRESHOLD + 1,
                    expected_decrease: SWEEPABLE_AMOUNTS[1],
                },
            ),
            (
                "the fee payer paid more than its transfer and the fee",
                SweepOutcome::of(&sweep).with_balances(
                    fee_payer,
                    RENT_EXEMPTION_THRESHOLD + SWEEPABLE_AMOUNTS[0] + 1,
                    RENT_EXEMPTION_THRESHOLD,
                ),
                SweepMismatch::UnexpectedBalanceChange {
                    address: fee_payer,
                    pre: RENT_EXEMPTION_THRESHOLD + SWEEPABLE_AMOUNTS[0] + 1,
                    post: RENT_EXEMPTION_THRESHOLD,
                    expected_decrease: SWEEPABLE_AMOUNTS[0],
                },
            ),
            (
                "a deposit address is left below the rent exemption threshold",
                SweepOutcome::of(&sweep).with_balances(
                    second_deposit,
                    SWEEPABLE_AMOUNTS[1] + RENT_EXEMPTION_THRESHOLD - 1,
                    RENT_EXEMPTION_THRESHOLD - 1,
                ),
                SweepMismatch::NotRentExempt {
                    address: second_deposit,
                    post: RENT_EXEMPTION_THRESHOLD - 1,
                },
            ),
            (
                "the main account was debited",
                SweepOutcome::of(&sweep).with_balances(
                    MINTER_ADDRESS,
                    MAIN_BALANCE,
                    MAIN_BALANCE - 1,
                ),
                SweepMismatch::MainAccountDebited {
                    pre: MAIN_BALANCE,
                    post: MAIN_BALANCE - 1,
                },
            ),
            (
                "the main account received less than planned",
                SweepOutcome::of(&sweep).with_balances(
                    MINTER_ADDRESS,
                    MAIN_BALANCE,
                    MAIN_BALANCE + sweep.expected_received() - 1,
                ),
                SweepMismatch::UnexpectedAmountReceived {
                    expected: sweep.expected_received(),
                    actual: sweep.expected_received() - 1,
                },
            ),
            (
                "the main account received more than planned",
                SweepOutcome::of(&sweep).with_balances(
                    MINTER_ADDRESS,
                    MAIN_BALANCE,
                    MAIN_BALANCE + sweep.expected_received() + 1,
                ),
                SweepMismatch::UnexpectedAmountReceived {
                    expected: sweep.expected_received(),
                    actual: sweep.expected_received() + 1,
                },
            ),
        ];

        for (name, outcome, expected) in cases {
            assert_eq!(
                sweep.clone().settle(&outcome.encode()),
                Err(SweepSettlementError::Mismatch(expected)),
                "{name}"
            );
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

#[test]
fn should_parse_a_real_world_sweep_transaction() {
    let transaction = transaction();

    let message = transaction
        .transaction
        .transaction
        .decode()
        .expect("decodable transaction")
        .message;
    assert_eq!(
        message.static_account_keys(),
        &[
            address!("FkEEvAwZNvSziMAkmt13z2L9Loy4MjJE2qDbRserzt5Z"),
            address!("6rXWHNpqRuNuGdUvJbRdV9wgRRggBVSEeCsnqJYDh7K9"),
            address!("AwgbdmwCfwc6qaZAVH2K5juuP7HL21kotkVreWym6Eq4"),
            address!("5tEJDWwGGG54bv2xzrphSieXSAYjLkxzdMEPGe53JaTj"),
            address!("11111111111111111111111111111111"),
        ]
    );
    let meta = transaction.transaction.meta.expect("meta");
    assert_eq!(meta.fee, 15_000);
    assert_eq!(
        meta.pre_balances,
        vec![1_499_990_000, 500_000_000, 500_000_000, 0, 1]
    );
    assert_eq!(
        meta.post_balances,
        vec![1_399_975_000, 300_000_000, 200_000_000, 600_000_000, 1]
    );
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

fn mutate_blob(
    mut outcome: EncodedConfirmedTransactionWithStatusMeta,
) -> EncodedConfirmedTransactionWithStatusMeta {
    match &mut outcome.transaction.transaction {
        EncodedTransaction::Binary(blob, _) => blob.push('!'),
        other => panic!("BUG: expected a binary transaction, got {other:?}"),
    }
    outcome
}

fn transaction() -> EncodedConfirmedTransactionWithStatusMeta {
    serde_json::from_value(json!({
      "blockTime": 1790688751,
      "meta": {
        "computeUnitsConsumed": 450,
        "costUnits": 3827,
        "err": null,
        "fee": 15000,
        "innerInstructions": [],
        "loadedAddresses": {
          "readonly": [],
          "writable": []
        },
        "logMessages": [
          "Program 11111111111111111111111111111111 invoke [1]",
          "Program 11111111111111111111111111111111 success",
          "Program 11111111111111111111111111111111 invoke [1]",
          "Program 11111111111111111111111111111111 success",
          "Program 11111111111111111111111111111111 invoke [1]",
          "Program 11111111111111111111111111111111 success"
        ],
        "postBalances": [
          1399975000,
          300000000,
          200000000,
          600000000,
          1
        ],
        "postTokenBalances": [],
        "preBalances": [
          1499990000,
          500000000,
          500000000,
          0,
          1
        ],
        "preTokenBalances": [],
        "rewards": [],
        "status": {
          "Ok": null
        }
      },
      "slot": 505546429,
      "transaction": [
        "A0l5aGzMiojZ7s3g857KdWKc5doZJEaXYR6pZcevgErpc9dLy9nwVnVfyPVnpB3bfpwF9HGQX4lIlcRuxArdtgdhxa5kqPjHR7e2Zks4KOCeS+cEAGQaFLDyNfy0TR9SlntM0eOts3e9CDQiZGgyqps7zYDBKGJlPb5JYHZ/Q/wOkIh/d7rIaqqROw1P4LoBXTcp+9s6KJKM6VF3dkXomwppY98UwX7VzNqbhEUjHwK0BANe8NxNk2pj0IEGH9tSAgMAAQXbFo/tF6Irs79HIQwkrXL2g5StGIhiLECAtszzWwuYzFb6YuCssGYBUTmCdb/38AL/yA3X2RJ8oD8BfyVye+uUk7tRk1tfdTVCsZXY9DqmOyL02mxVyqvO8TNExRK7+P9IjmdbL31Vgaf52SlZWAP86sW6R0T6zA2hJjN7zYaxugAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAb17pv0rswYcGIrcVc+2uZXToaVhHVtvsVkd9awK87ygDBAIAAwwCAAAAAOH1BQAAAAAEAgEDDAIAAAAAwusLAAAAAAQCAgMMAgAAAACj4REAAAAA",
        "base64"
      ],
      "transactionIndex": 14,
      "version": "legacy"
    }))
    .expect("BUG: the getTransaction result should deserialize")
}
