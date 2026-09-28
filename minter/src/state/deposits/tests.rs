use super::{DepositBalance, Deposits, MintedSweep, PendingMint, QueuedDeposit, SweptDeposit};
use crate::{
    constants::{FEE_PER_SIGNATURE, RENT_EXEMPTION_THRESHOLD},
    numeric::LedgerMintIndex,
    state::event::CreditedDeposit,
    test_fixtures::{
        MINTER_ADDRESS, planned_sweep, queued_deposit, queued_deposit_of, signature, sweep_message,
    },
};
use cksol_types::{DepositSolId, DepositSolStatus};
use sol_rpc_types::Lamport;
use std::collections::BTreeMap;

const SWEEP_SIGNATURE_INDEX: usize = 0xAA;
const CREDIT_TIMESTAMP: u64 = 1_234;

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

            assert_eq!(Lamport::from(deposit_balance), balance, "balance {balance}");
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
        BTreeMap, DepositSolStatus, Deposits, FEE_PER_SIGNATURE, MINTER_ADDRESS,
        SWEEP_SIGNATURE_INDEX, planned_sweep, queued_deposit, signature, sweep_message,
    };

    #[test]
    fn should_move_queued_deposits_to_a_sweep_and_return_the_expected_received_amount() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));
        deposits.queue(2, queued_deposit(2));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);

        let expected_received = deposits.sweep(
            &[2, 0],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0)), (2, queued_deposit(2))]),
            &sweep_signature,
        );

        assert_eq!(
            expected_received,
            queued_deposit(2).sweepable_amount() + queued_deposit(0).sweepable_amount()
                - 2 * FEE_PER_SIGNATURE
        );
        assert_eq!(
            deposits.swept().get(&sweep_signature),
            Some(&planned_sweep([
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
        Deposits::default().sweep(
            &[3],
            MINTER_ADDRESS,
            &sweep_message([(3, queued_deposit(3))]),
            &signature(SWEEP_SIGNATURE_INDEX),
        );
    }

    #[test]
    #[should_panic(expected = "Attempted to sweep unknown or already swept deposit 0")]
    fn should_panic_when_sweeping_already_swept_deposit() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.sweep(
            &[0],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0))]),
            &signature(SWEEP_SIGNATURE_INDEX),
        );

        deposits.sweep(
            &[0],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0))]),
            &signature(SWEEP_SIGNATURE_INDEX + 1),
        );
    }

    #[test]
    #[should_panic(expected = "the plan builds")]
    fn should_panic_when_the_message_does_not_match_the_plan() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));

        deposits.sweep(
            &[0, 1],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0))]),
            &signature(SWEEP_SIGNATURE_INDEX),
        );
    }

    #[test]
    #[should_panic(expected = "Attempted to sweep no deposits")]
    fn should_panic_when_sweeping_no_deposits() {
        Deposits::default().sweep(
            &[],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0))]),
            &signature(SWEEP_SIGNATURE_INDEX),
        );
    }

    #[test]
    #[should_panic(expected = "Attempted to record sweep")]
    fn should_panic_when_reusing_a_sweep_signature() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[0],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0))]),
            &sweep_signature,
        );

        deposits.sweep(
            &[1],
            MINTER_ADDRESS,
            &sweep_message([(1, queued_deposit(1))]),
            &sweep_signature,
        );
    }
}

mod drop_swept {
    use super::{
        BTreeMap, DepositSolStatus, Deposits, MINTER_ADDRESS, SWEEP_SIGNATURE_INDEX, SweptDeposit,
        queue_deposits, signature, sweep_message,
    };

    #[test]
    fn should_move_the_deposits_of_the_sweep_to_dropped_and_release_their_accounts() {
        let mut deposits = Deposits::default();
        let [first, second, third] = queue_deposits(&mut deposits);
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[2, 0],
            MINTER_ADDRESS,
            &sweep_message([(0, first), (2, third)]),
            &sweep_signature,
        );

        deposits.drop_swept(&sweep_signature);

        assert!(deposits.swept().is_empty());
        let dropped = |deposit| SweptDeposit {
            deposit,
            signature: sweep_signature,
        };
        assert_eq!(
            deposits.dropped(),
            &BTreeMap::from([(0, dropped(first)), (2, dropped(third))])
        );
        for (deposit_id, deposit) in [(0, first), (2, third)] {
            assert_eq!(deposits.in_flight_id(&deposit.account), None);
            assert_eq!(
                deposits.status(deposit_id),
                DepositSolStatus::Dropped {
                    signature: sweep_signature.into()
                }
            );
        }
        assert_eq!(deposits.in_flight_id(&second.account), Some(1));
    }

    #[test]
    #[should_panic(expected = "Attempted to drop sweep")]
    fn should_panic_when_dropping_a_sweep_that_is_not_swept() {
        Deposits::default().drop_swept(&signature(SWEEP_SIGNATURE_INDEX));
    }
}

mod finalize_swept {
    use super::{
        BTreeMap, DepositSolStatus, Deposits, MINTER_ADDRESS, SWEEP_SIGNATURE_INDEX, planned_sweep,
        queued_deposit, signature, sweep_message,
    };

    #[test]
    fn should_move_the_sweep_to_finalized() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));
        deposits.queue(2, queued_deposit(2));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[2, 0],
            MINTER_ADDRESS,
            &sweep_message([(2, queued_deposit(2)), (0, queued_deposit(0))]),
            &sweep_signature,
        );

        deposits.finalize_swept(&sweep_signature);

        assert!(deposits.swept().is_empty());
        assert_eq!(
            deposits.finalized().get(&sweep_signature),
            Some(&planned_sweep([
                (0, queued_deposit(0)),
                (2, queued_deposit(2))
            ]))
        );
        assert_eq!(deposits.queued(), &BTreeMap::from([(1, queued_deposit(1))]));
        for deposit_id in 0..3 {
            assert_eq!(
                deposits.in_flight_id(&queued_deposit(deposit_id).account),
                Some(deposit_id)
            );
        }
        for deposit_id in [0, 2] {
            assert_eq!(
                deposits.status(deposit_id),
                DepositSolStatus::Finalized {
                    signature: sweep_signature.into()
                }
            );
        }
        assert_eq!(
            deposits.status(1),
            DepositSolStatus::Queued {
                sweepable_amount: queued_deposit(1).sweepable_amount()
            }
        );
    }

    #[test]
    #[should_panic(expected = "Attempted to finalize sweep")]
    fn should_panic_when_finalizing_a_sweep_that_is_not_swept() {
        Deposits::default().finalize_swept(&signature(SWEEP_SIGNATURE_INDEX));
    }
}

mod credit_sweep {
    use super::{
        BTreeMap, CREDIT_TIMESTAMP, DepositSolStatus, Deposits, MINTER_ADDRESS, PendingMint,
        SWEEP_SIGNATURE_INDEX, SweptDeposit, mint, queued_deposit, signature, sweep_message,
    };

    #[test]
    fn should_move_the_deposits_of_the_sweep_to_pending_mints_with_the_given_amounts() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));
        deposits.queue(2, queued_deposit(2));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[2, 0],
            MINTER_ADDRESS,
            &sweep_message([(2, queued_deposit(2)), (0, queued_deposit(0))]),
            &sweep_signature,
        );
        deposits.finalize_swept(&sweep_signature);

        deposits.credit_sweep(
            &sweep_signature,
            &[mint(2, 295), mint(0, 95)],
            CREDIT_TIMESTAMP,
        );

        assert!(deposits.finalized().is_empty());
        let pending_mint = |deposit_id, amount_to_mint| PendingMint {
            deposit: SweptDeposit {
                deposit: queued_deposit(deposit_id),
                signature: sweep_signature,
            },
            amount_to_mint,
            created_at_time: CREDIT_TIMESTAMP,
        };
        assert_eq!(
            deposits.pending_mints(),
            &BTreeMap::from([(0, pending_mint(0, 95)), (2, pending_mint(2, 295))])
        );
        for deposit_id in 0..3 {
            assert_eq!(
                deposits.in_flight_id(&queued_deposit(deposit_id).account),
                Some(deposit_id)
            );
        }
        assert_eq!(
            deposits.status(0),
            DepositSolStatus::Finalized {
                signature: sweep_signature.into()
            }
        );
    }

    #[test]
    #[should_panic(expected = "Attempted to credit sweep")]
    fn should_panic_when_the_sweep_is_not_finalized() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[0],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0))]),
            &sweep_signature,
        );

        deposits.credit_sweep(&sweep_signature, &[mint(0, 100)], CREDIT_TIMESTAMP);
    }

    #[test]
    #[should_panic(expected = "with 1 mints for 2 deposits")]
    fn should_panic_when_a_deposit_of_the_sweep_has_no_mint() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[0, 1],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0)), (1, queued_deposit(1))]),
            &sweep_signature,
        );
        deposits.finalize_swept(&sweep_signature);

        deposits.credit_sweep(&sweep_signature, &[mint(1, 200)], CREDIT_TIMESTAMP);
    }

    #[test]
    #[should_panic(expected = "is not part of sweep")]
    fn should_panic_when_a_mint_is_for_a_deposit_of_another_sweep() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));
        deposits.queue(2, queued_deposit(2));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[0, 2],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0)), (2, queued_deposit(2))]),
            &sweep_signature,
        );
        deposits.finalize_swept(&sweep_signature);

        deposits.credit_sweep(
            &sweep_signature,
            &[mint(0, 100), mint(1, 200)],
            CREDIT_TIMESTAMP,
        );
    }

    #[test]
    #[should_panic(expected = "Attempted to credit deposit 0 twice")]
    fn should_panic_when_a_deposit_is_minted_twice() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[0, 1],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0)), (1, queued_deposit(1))]),
            &sweep_signature,
        );
        deposits.finalize_swept(&sweep_signature);

        deposits.credit_sweep(
            &sweep_signature,
            &[mint(0, 100), mint(0, 100)],
            CREDIT_TIMESTAMP,
        );
    }

    #[test]
    #[should_panic(expected = "beyond its sweepable amount")]
    fn should_panic_when_a_mint_exceeds_the_sweepable_amount() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[0],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0))]),
            &sweep_signature,
        );
        deposits.finalize_swept(&sweep_signature);

        deposits.credit_sweep(
            &sweep_signature,
            &[mint(0, queued_deposit(0).sweepable_amount() + 1)],
            CREDIT_TIMESTAMP,
        );
    }
}

mod mint {
    use super::{
        BTreeMap, CREDIT_TIMESTAMP, DepositSolStatus, Deposits, LedgerMintIndex, MINTER_ADDRESS,
        MintedSweep, SWEEP_SIGNATURE_INDEX, SweptDeposit, mint, queued_deposit, signature,
        sweep_message,
    };

    #[test]
    fn should_move_the_pending_mint_to_minted_and_release_its_account() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[0, 1],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0)), (1, queued_deposit(1))]),
            &sweep_signature,
        );
        deposits.finalize_swept(&sweep_signature);
        deposits.credit_sweep(
            &sweep_signature,
            &[mint(0, 100), mint(1, 200)],
            CREDIT_TIMESTAMP,
        );

        deposits.mint(0, LedgerMintIndex::from(42));

        assert_eq!(
            deposits.pending_mints().keys().collect::<Vec<_>>(),
            vec![&1]
        );
        assert_eq!(
            deposits.minted(),
            &BTreeMap::from([(
                0,
                MintedSweep {
                    deposit: SweptDeposit {
                        deposit: queued_deposit(0),
                        signature: sweep_signature,
                    },
                    minted_amount: 100,
                    mint_block_index: LedgerMintIndex::from(42),
                }
            )])
        );
        assert_eq!(deposits.in_flight_id(&queued_deposit(0).account), None);
        assert_eq!(deposits.in_flight_id(&queued_deposit(1).account), Some(1));
        assert_eq!(
            deposits.status(0),
            DepositSolStatus::Minted {
                block_index: 42,
                minted_amount: 100,
            }
        );
        assert_eq!(
            deposits.status(1),
            DepositSolStatus::Finalized {
                signature: sweep_signature.into()
            }
        );
    }

    #[test]
    #[should_panic(expected = "Attempted to mint deposit 0 that has no pending mint")]
    fn should_panic_without_pending_mint() {
        Deposits::default().mint(0, LedgerMintIndex::from(42));
    }
}

mod quarantine_pending_mint {
    use super::{
        CREDIT_TIMESTAMP, DepositSolStatus, Deposits, MINTER_ADDRESS, SWEEP_SIGNATURE_INDEX, mint,
        queued_deposit, signature, sweep_message,
    };

    #[test]
    fn should_move_the_pending_mint_to_quarantined_without_releasing_its_account() {
        let mut deposits = Deposits::default();
        deposits.queue(0, queued_deposit(0));
        deposits.queue(1, queued_deposit(1));
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[0, 1],
            MINTER_ADDRESS,
            &sweep_message([(0, queued_deposit(0)), (1, queued_deposit(1))]),
            &sweep_signature,
        );
        deposits.finalize_swept(&sweep_signature);
        deposits.credit_sweep(
            &sweep_signature,
            &[mint(0, 100), mint(1, 200)],
            CREDIT_TIMESTAMP,
        );

        deposits.quarantine_pending_mint(0);

        assert_eq!(
            deposits.pending_mints().keys().collect::<Vec<_>>(),
            vec![&1]
        );
        assert!(deposits.minted().is_empty());
        assert_eq!(deposits.quarantined().keys().collect::<Vec<_>>(), vec![&0]);
        for deposit_id in 0..2 {
            assert_eq!(
                deposits.in_flight_id(&queued_deposit(deposit_id).account),
                Some(deposit_id)
            );
        }
        assert_eq!(
            deposits.status(0),
            DepositSolStatus::Quarantined {
                signature: sweep_signature.into()
            }
        );
    }

    #[test]
    #[should_panic(expected = "Attempted to quarantine deposit 0 that has no pending mint")]
    fn should_panic_without_pending_mint() {
        Deposits::default().quarantine_pending_mint(0);
    }
}

mod quarantine_sweep {
    use super::{
        BTreeMap, DepositSolStatus, Deposits, MINTER_ADDRESS, SWEEP_SIGNATURE_INDEX, SweptDeposit,
        queue_deposits, signature, sweep_message,
    };

    #[test]
    fn should_move_the_deposits_of_the_sweep_to_quarantined_and_keep_their_accounts_in_flight() {
        let mut deposits = Deposits::default();
        let [first, second, third] = queue_deposits(&mut deposits);
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[2, 0],
            MINTER_ADDRESS,
            &sweep_message([(2, third), (0, first)]),
            &sweep_signature,
        );
        deposits.finalize_swept(&sweep_signature);

        deposits.quarantine_sweep(&sweep_signature);

        assert!(deposits.finalized().is_empty());
        assert!(deposits.pending_mints().is_empty());
        let quarantined = |deposit| SweptDeposit {
            deposit,
            signature: sweep_signature,
        };
        assert_eq!(
            deposits.quarantined(),
            &BTreeMap::from([(0, quarantined(first)), (2, quarantined(third))])
        );
        for (deposit_id, deposit) in [(0, first), (1, second), (2, third)] {
            assert_eq!(deposits.in_flight_id(&deposit.account), Some(deposit_id));
        }
        for deposit_id in [0, 2] {
            assert_eq!(
                deposits.status(deposit_id),
                DepositSolStatus::Quarantined {
                    signature: sweep_signature.into()
                }
            );
        }
    }

    #[test]
    #[should_panic(expected = "Attempted to quarantine sweep")]
    fn should_panic_when_the_sweep_is_not_finalized() {
        let mut deposits = Deposits::default();
        let [deposit] = queue_deposits(&mut deposits);
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        deposits.sweep(
            &[0],
            MINTER_ADDRESS,
            &sweep_message([(0, deposit)]),
            &sweep_signature,
        );

        deposits.quarantine_sweep(&sweep_signature);
    }
}

/// Queues `N` distinct deposits under the ids `0..N` and returns them in that order.
fn queue_deposits<const N: usize>(deposits: &mut Deposits) -> [QueuedDeposit; N] {
    std::array::from_fn(|index| {
        let deposit = queued_deposit(index as DepositSolId);
        deposits.queue(index as DepositSolId, deposit);
        deposit
    })
}

fn mint(deposit_id: DepositSolId, amount_to_mint: Lamport) -> CreditedDeposit {
    CreditedDeposit {
        deposit_id,
        amount_to_mint,
    }
}
