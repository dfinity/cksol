use super::{
    SweepMetadataError, SweepMismatch, UnreadableMetadata, amount_received_by,
    credit_finalized_sweeps,
};
use crate::{
    address::account_address,
    constants::{FEE_PER_SIGNATURE, MAX_CONCURRENT_RPC_CALLS, RENT_EXEMPTION_THRESHOLD},
    state::{PendingMint, QueuedDeposit, SweptDeposit, event::EventType, read_state},
    test_fixtures::{
        EventsAssert, MINTER_ADDRESS, account,
        events::{queue_deposit, submit_sweep, succeed_transaction},
        init_schnorr_master_key, init_state,
        runtime::TestCanisterRuntime,
        signature,
    },
};
use base64::{Engine, engine::general_purpose::STANDARD};
use cksol_types::{DepositSolId, DepositSolStatus};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{Lamport, MultiRpcResult};
use solana_address::Address;
use solana_hash::Hash;
use solana_message::Message;
use solana_system_interface::instruction::transfer;
use solana_transaction::{Transaction, versioned::VersionedTransaction};
use solana_transaction_status_client_types::{
    EncodedConfirmedTransactionWithStatusMeta, EncodedTransaction,
    EncodedTransactionWithStatusMeta, TransactionBinaryEncoding, UiTransactionStatusMeta,
    option_serializer::OptionSerializer,
};
use std::collections::BTreeMap;

type GetTransactionResult =
    MultiRpcResult<Option<sol_rpc_types::EncodedConfirmedTransactionWithStatusMeta>>;

const MAIN_BALANCE: Lamport = 7_000_000_000;
const SWEEPABLE_AMOUNTS: [Lamport; 3] = [30_000_000, 20_000_000, 10_000_000];
const ASSUMED_FEE: Lamport = FEE_PER_SIGNATURE * SWEEPABLE_AMOUNTS.len() as u64;

fn swept_amount() -> Lamport {
    SWEEPABLE_AMOUNTS.iter().sum()
}

fn other_address() -> Address {
    Address::from([0x77; 32])
}

mod amount_received {
    use super::*;

    /// Addresses standing in for the deposit addresses of a three-deposit sweep.
    fn swept_addresses() -> Vec<(Address, Lamport)> {
        SWEEPABLE_AMOUNTS
            .iter()
            .enumerate()
            .map(|(index, sweepable_amount)| {
                (Address::from([index as u8 + 1; 32]), *sweepable_amount)
            })
            .collect()
    }

    #[test]
    fn should_report_the_increase_of_the_main_account_balance() {
        let swept = swept_addresses();
        let cases = [
            (
                "the fee was as assumed",
                sweep_transaction(&swept),
                swept_amount() - ASSUMED_FEE,
            ),
            (
                "a late transfer left extra on a deposit address",
                sweep_transaction(&swept).with_late_transfer(swept[1].0, 123_456),
                swept_amount() - ASSUMED_FEE,
            ),
            (
                "a lower fee left extra on the fee payer",
                sweep_transaction(&swept).with_fee(ASSUMED_FEE - 1_000),
                swept_amount() - ASSUMED_FEE,
            ),
            (
                "the whole swept amount arrived",
                sweep_transaction(&swept).with_balances(
                    MINTER_ADDRESS,
                    MAIN_BALANCE,
                    MAIN_BALANCE + swept_amount(),
                ),
                swept_amount(),
            ),
            (
                "more than the swept amount arrived",
                sweep_transaction(&swept).with_balances(
                    MINTER_ADDRESS,
                    MAIN_BALANCE,
                    MAIN_BALANCE + swept_amount() + 7,
                ),
                swept_amount() + 7,
            ),
        ];

        for (name, transaction, expected) in cases {
            assert_eq!(
                amount_received_by(&transaction.encode(), MINTER_ADDRESS, &swept),
                Ok(expected),
                "{name}"
            );
        }
    }

    #[test]
    fn should_reject_metadata_that_does_not_match_the_sweep() {
        let swept = swept_addresses();
        let wrong_amount = swept[2].0;
        let below_threshold = swept[1].0;
        let cases = [
            (
                "no meta field",
                sweep_transaction(&swept).encode_without_meta(),
                swept.clone(),
                SweepMetadataError::Unreadable(UnreadableMetadata::NoMetaField),
            ),
            (
                "the main account is not part of the transaction",
                sweep_transaction_to(&swept, other_address()).encode(),
                swept.clone(),
                SweepMetadataError::Mismatch(SweepMismatch::AddressNotInTransaction {
                    address: MINTER_ADDRESS,
                }),
            ),
            (
                "a deposit address is not part of the transaction",
                sweep_transaction(&swept).encode(),
                vec![(other_address(), 1_000_000)],
                SweepMetadataError::Mismatch(SweepMismatch::AddressNotInTransaction {
                    address: other_address(),
                }),
            ),
            (
                "a deposit address moved by the wrong amount",
                sweep_transaction(&swept)
                    .with_balances(
                        wrong_amount,
                        RENT_EXEMPTION_THRESHOLD + SWEEPABLE_AMOUNTS[2],
                        RENT_EXEMPTION_THRESHOLD + 1,
                    )
                    .encode(),
                swept.clone(),
                SweepMetadataError::Mismatch(SweepMismatch::UnexpectedBalanceChange {
                    address: wrong_amount,
                    pre: RENT_EXEMPTION_THRESHOLD + SWEEPABLE_AMOUNTS[2],
                    post: RENT_EXEMPTION_THRESHOLD + 1,
                    expected_decrease: SWEEPABLE_AMOUNTS[2],
                }),
            ),
            (
                "a deposit address is left below the rent exemption threshold",
                sweep_transaction(&swept)
                    .with_balances(below_threshold, SWEEPABLE_AMOUNTS[1], 0)
                    .encode(),
                swept.clone(),
                SweepMetadataError::Mismatch(SweepMismatch::NotRentExempt {
                    address: below_threshold,
                    post: 0,
                }),
            ),
            (
                "the main account was debited",
                sweep_transaction(&swept)
                    .with_balances(MINTER_ADDRESS, MAIN_BALANCE, MAIN_BALANCE - 1)
                    .encode(),
                swept.clone(),
                SweepMetadataError::Mismatch(SweepMismatch::MainAccountDebited {
                    pre: MAIN_BALANCE,
                    post: MAIN_BALANCE - 1,
                }),
            ),
            (
                "the main account received less than the swept amount minus the assumed fee",
                sweep_transaction(&swept)
                    .with_balances(
                        MINTER_ADDRESS,
                        MAIN_BALANCE,
                        MAIN_BALANCE + swept_amount() - ASSUMED_FEE - 1,
                    )
                    .encode(),
                swept.clone(),
                SweepMetadataError::Mismatch(SweepMismatch::AmountReceivedTooSmall {
                    swept_amount: swept_amount(),
                    amount_received: swept_amount() - ASSUMED_FEE - 1,
                    assumed_fee: ASSUMED_FEE,
                }),
            ),
        ];

        for (name, transaction, swept, expected) in cases {
            assert_eq!(
                amount_received_by(&transaction, MINTER_ADDRESS, &swept),
                Err(expected),
                "{name}"
            );
        }
    }
}

mod credit {
    use super::*;

    const SWEEP_SIGNATURE_INDEX: usize = 0xAA;

    #[tokio::test]
    async fn should_do_nothing_without_finalized_deposits() {
        setup();

        let run_again = credit_finalized_sweeps(&TestCanisterRuntime::new()).await;

        assert!(!run_again);
        EventsAssert::assert_no_events_recorded();
    }

    #[tokio::test]
    async fn should_ask_for_another_round_when_sweeps_are_left_over() {
        const SWEEPABLE_AMOUNT: Lamport = 25_000_000;
        setup();
        let sweeps = MAX_CONCURRENT_RPC_CALLS + 1;
        let mut runtime = TestCanisterRuntime::new().with_increasing_time();
        for index in 0..sweeps {
            let deposit_id = index as DepositSolId;
            queue_deposit(deposit_id, account(index), SWEEPABLE_AMOUNT);
            let sweep_signature = signature(index);
            submit_sweep(sweep_signature, vec![deposit_id]);
            succeed_transaction(sweep_signature);
            if index < MAX_CONCURRENT_RPC_CALLS {
                let swept = vec![(deposit_address(account(index)), SWEEPABLE_AMOUNT)];
                runtime = with_transaction(runtime, sweep_transaction(&swept).encode());
            }
        }

        let run_again = credit_finalized_sweeps(&runtime).await;

        assert!(run_again);
        read_state(|state| {
            assert_eq!(state.pending_mints().len(), MAX_CONCURRENT_RPC_CALLS);
            assert_eq!(state.finalized_deposits().len(), 1);
        });
    }

    #[tokio::test]
    async fn should_enqueue_one_pending_mint_per_deposit() {
        setup();
        let sweep_signature = queue_finalized_sweep();
        let runtime = runtime_returning(sweep_transaction(&swept_addresses()).encode());

        credit_finalized_sweeps(&runtime).await;

        let shortfall_share = ASSUMED_FEE.div_ceil(SWEEPABLE_AMOUNTS.len() as u64);
        read_state(|state| {
            assert!(state.finalized_deposits().is_empty());
            assert_eq!(state.balance(), swept_amount() - ASSUMED_FEE);
            assert_eq!(
                state.pending_mints(),
                &expected_pending_mints(sweep_signature, shortfall_share)
            );
        });
        for deposit_id in 0..SWEEPABLE_AMOUNTS.len() as DepositSolId {
            assert_eq!(
                deposit_status(deposit_id),
                DepositSolStatus::Finalized {
                    signature: sweep_signature.into()
                }
            );
        }
        EventsAssert::from_recorded().expect_contains_event_eq(EventType::CreditedSweep {
            signature: sweep_signature,
            amount_received: swept_amount() - ASSUMED_FEE,
        });
    }

    #[tokio::test]
    async fn should_round_the_shortfall_share_up() {
        setup();
        queue_finalized_sweep();
        let shortfall = 10_001;
        let amount_received = swept_amount() - shortfall;
        let runtime = runtime_returning(
            sweep_transaction(&swept_addresses())
                .with_balances(MINTER_ADDRESS, MAIN_BALANCE, MAIN_BALANCE + amount_received)
                .encode(),
        );

        credit_finalized_sweeps(&runtime).await;

        let shortfall_share = shortfall.div_ceil(SWEEPABLE_AMOUNTS.len() as u64);
        assert_eq!(shortfall_share, 3_334);
        let minted = total_amount_to_mint();
        assert_eq!(
            minted,
            swept_amount() - SWEEPABLE_AMOUNTS.len() as u64 * shortfall_share
        );
        assert!(minted <= amount_received);
    }

    #[tokio::test]
    async fn should_mint_the_full_sweepable_amounts_when_nothing_is_missing() {
        setup();
        queue_finalized_sweep();
        let runtime = runtime_returning(
            sweep_transaction(&swept_addresses())
                .with_balances(MINTER_ADDRESS, MAIN_BALANCE, MAIN_BALANCE + swept_amount())
                .encode(),
        );

        credit_finalized_sweeps(&runtime).await;

        assert_eq!(total_amount_to_mint(), swept_amount());
        read_state(|state| assert_eq!(state.balance(), swept_amount()));
    }

    #[tokio::test]
    async fn should_keep_deposits_finalized_if_fetching_the_transaction_fails() {
        setup();
        let sweep_signature = queue_finalized_sweep();
        let events_before = EventsAssert::from_recorded();
        let runtime = TestCanisterRuntime::new()
            .with_increasing_time()
            .add_stub_response(GetTransactionResult::Inconsistent(vec![]));

        credit_finalized_sweeps(&runtime).await;

        assert_eq!(events_before, EventsAssert::from_recorded());
        read_state(|state| {
            assert_eq!(state.finalized_deposits().len(), SWEEPABLE_AMOUNTS.len());
            assert!(state.pending_mints().is_empty());
        });
        assert_eq!(
            deposit_status(0),
            DepositSolStatus::Finalized {
                signature: sweep_signature.into()
            }
        );
    }

    #[tokio::test]
    async fn should_keep_deposits_finalized_if_the_metadata_cannot_be_read() {
        setup();
        let sweep_signature = queue_finalized_sweep();
        let events_before = EventsAssert::from_recorded();
        let runtime =
            runtime_returning(sweep_transaction(&swept_addresses()).encode_without_meta());

        credit_finalized_sweeps(&runtime).await;

        assert_eq!(events_before, EventsAssert::from_recorded());
        read_state(|state| {
            assert_eq!(state.finalized_deposits().len(), SWEEPABLE_AMOUNTS.len());
            assert!(state.quarantined_swept_deposits().is_empty());
        });
        assert_eq!(
            deposit_status(0),
            DepositSolStatus::Finalized {
                signature: sweep_signature.into()
            }
        );
    }

    #[tokio::test]
    async fn should_quarantine_deposits_if_too_little_arrived_on_the_main_account() {
        setup();
        let sweep_signature = queue_finalized_sweep();
        let runtime = runtime_returning(
            sweep_transaction(&swept_addresses())
                .with_balances(MINTER_ADDRESS, MAIN_BALANCE, MAIN_BALANCE)
                .encode(),
        );

        credit_finalized_sweeps(&runtime).await;

        read_state(|state| {
            assert!(state.pending_mints().is_empty());
            assert_eq!(
                state.quarantined_swept_deposits().len(),
                SWEEPABLE_AMOUNTS.len()
            );
            assert_eq!(state.balance(), 0);
        });
        assert_eq!(
            deposit_status(0),
            DepositSolStatus::Quarantined {
                signature: sweep_signature.into()
            }
        );
    }

    #[tokio::test]
    async fn should_quarantine_deposits_if_the_metadata_is_unexpected() {
        setup();
        let sweep_signature = queue_finalized_sweep();
        let runtime =
            runtime_returning(sweep_transaction_to(&swept_addresses(), other_address()).encode());

        credit_finalized_sweeps(&runtime).await;

        read_state(|state| {
            assert!(state.finalized_deposits().is_empty());
            assert!(state.pending_mints().is_empty());
            assert_eq!(
                state.quarantined_swept_deposits().len(),
                SWEEPABLE_AMOUNTS.len()
            );
            assert_eq!(state.balance(), 0);
        });
        for deposit_id in 0..SWEEPABLE_AMOUNTS.len() as DepositSolId {
            assert_eq!(
                deposit_status(deposit_id),
                DepositSolStatus::Quarantined {
                    signature: sweep_signature.into()
                }
            );
            assert_eq!(
                read_state(|state| state.in_flight_deposit_id(&account(deposit_id as usize))),
                Some(deposit_id),
                "a quarantined deposit keeps the account reserved"
            );
        }
        EventsAssert::from_recorded().expect_contains_event_eq(EventType::QuarantinedSweep {
            signature: sweep_signature,
        });
    }

    fn setup() {
        init_state();
        init_schnorr_master_key();
    }

    /// The deposit addresses of the deposits queued by [`queue_finalized_sweep`].
    fn swept_addresses() -> Vec<(Address, Lamport)> {
        SWEEPABLE_AMOUNTS
            .iter()
            .enumerate()
            .map(|(index, sweepable_amount)| (deposit_address(account(index)), *sweepable_amount))
            .collect()
    }

    fn deposit_address(account: Account) -> Address {
        let master_key = read_state(|state| state.minter_public_key().cloned())
            .expect("the master key should be initialized");
        account_address(&master_key, &account)
    }

    fn queue_finalized_sweep() -> solana_signature::Signature {
        for (deposit_id, sweepable_amount) in SWEEPABLE_AMOUNTS.iter().enumerate() {
            queue_deposit(
                deposit_id as DepositSolId,
                account(deposit_id),
                *sweepable_amount,
            );
        }
        let sweep_signature = signature(SWEEP_SIGNATURE_INDEX);
        submit_sweep(
            sweep_signature,
            (0..SWEEPABLE_AMOUNTS.len() as DepositSolId).collect(),
        );
        succeed_transaction(sweep_signature);
        sweep_signature
    }

    fn expected_pending_mints(
        signature: solana_signature::Signature,
        shortfall_share: Lamport,
    ) -> BTreeMap<DepositSolId, PendingMint> {
        SWEEPABLE_AMOUNTS
            .iter()
            .enumerate()
            .map(|(deposit_id, sweepable_amount)| {
                (
                    deposit_id as DepositSolId,
                    PendingMint {
                        deposit: SweptDeposit {
                            deposit: QueuedDeposit {
                                account: account(deposit_id),
                                sweepable_amount: *sweepable_amount,
                            },
                            signature,
                        },
                        amount_to_mint: sweepable_amount - shortfall_share,
                    },
                )
            })
            .collect()
    }

    fn total_amount_to_mint() -> Lamport {
        read_state(|state| {
            state
                .pending_mints()
                .values()
                .map(|pending| pending.amount_to_mint)
                .sum()
        })
    }

    fn runtime_returning(
        transaction: EncodedConfirmedTransactionWithStatusMeta,
    ) -> TestCanisterRuntime {
        with_transaction(
            TestCanisterRuntime::new().with_increasing_time(),
            transaction,
        )
    }

    fn with_transaction(
        runtime: TestCanisterRuntime,
        transaction: EncodedConfirmedTransactionWithStatusMeta,
    ) -> TestCanisterRuntime {
        runtime.add_stub_response(GetTransactionResult::Consistent(Ok(Some(
            transaction
                .try_into()
                .expect("failed to convert transaction"),
        ))))
    }

    fn deposit_status(deposit_id: DepositSolId) -> DepositSolStatus {
        read_state(|state| state.deposit_sol_status(deposit_id))
    }
}

fn sweep_transaction(swept: &[(Address, Lamport)]) -> SweepTransaction {
    sweep_transaction_to(swept, MINTER_ADDRESS)
}

fn sweep_transaction_to(swept: &[(Address, Lamport)], main_address: Address) -> SweepTransaction {
    SweepTransaction::new(swept.to_vec(), main_address)
}

/// Builds the `getTransaction` response of a sweep whose transfers all executed and
/// whose deposit addresses are left rent-exempt, so that a test only has to state how
/// it deviates from that.
struct SweepTransaction {
    swept: Vec<(Address, Lamport)>,
    main_address: Address,
    fee: Lamport,
    balances: BTreeMap<Address, (Lamport, Lamport)>,
}

impl SweepTransaction {
    fn new(swept: Vec<(Address, Lamport)>, main_address: Address) -> Self {
        let fee = FEE_PER_SIGNATURE * swept.len() as u64;
        let transferred: Lamport = swept.iter().map(|(_, amount)| amount).sum::<Lamport>() - fee;
        let mut balances: BTreeMap<Address, (Lamport, Lamport)> = swept
            .iter()
            .map(|(address, sweepable_amount)| {
                (
                    *address,
                    (
                        RENT_EXEMPTION_THRESHOLD + sweepable_amount,
                        RENT_EXEMPTION_THRESHOLD,
                    ),
                )
            })
            .collect();
        balances.insert(main_address, (MAIN_BALANCE, MAIN_BALANCE + transferred));
        Self {
            swept,
            main_address,
            fee,
            balances,
        }
    }

    fn with_fee(mut self, fee: Lamport) -> Self {
        let (fee_payer, _) = self.swept[0];
        let (pre, _) = self.balances[&fee_payer];
        let assumed_fee = FEE_PER_SIGNATURE * self.swept.len() as u64;
        self.balances.insert(
            fee_payer,
            (pre, RENT_EXEMPTION_THRESHOLD + assumed_fee - fee),
        );
        self.fee = fee;
        self
    }

    fn with_late_transfer(mut self, address: Address, amount: Lamport) -> Self {
        let (pre, post) = self.balances[&address];
        self.balances.insert(address, (pre + amount, post + amount));
        self
    }

    fn with_balances(mut self, address: Address, pre: Lamport, post: Lamport) -> Self {
        self.balances.insert(address, (pre, post));
        self
    }

    fn encode(&self) -> EncodedConfirmedTransactionWithStatusMeta {
        self.encode_with_meta(Some(self.meta()))
    }

    fn encode_without_meta(&self) -> EncodedConfirmedTransactionWithStatusMeta {
        self.encode_with_meta(None)
    }

    fn message(&self) -> Message {
        let assumed_fee = FEE_PER_SIGNATURE * self.swept.len() as u64;
        let instructions: Vec<_> = self
            .swept
            .iter()
            .enumerate()
            .map(|(index, (address, sweepable_amount))| {
                let transfer_amount = if index == 0 {
                    sweepable_amount - assumed_fee
                } else {
                    *sweepable_amount
                };
                transfer(address, &self.main_address, transfer_amount)
            })
            .collect();
        Message::new_with_blockhash(&instructions, Some(&self.swept[0].0), &Hash::default())
    }

    fn meta(&self) -> UiTransactionStatusMeta {
        let (pre_balances, post_balances) = self
            .message()
            .account_keys
            .iter()
            .map(|key| self.balances.get(key).copied().unwrap_or((1, 1)))
            .unzip();
        UiTransactionStatusMeta {
            err: None,
            fee: self.fee,
            pre_balances,
            post_balances,
            status: Ok(()),
            inner_instructions: OptionSerializer::Skip,
            log_messages: OptionSerializer::Skip,
            pre_token_balances: OptionSerializer::Skip,
            post_token_balances: OptionSerializer::Skip,
            rewards: OptionSerializer::Skip,
            loaded_addresses: OptionSerializer::Skip,
            return_data: OptionSerializer::Skip,
            compute_units_consumed: OptionSerializer::Skip,
            cost_units: OptionSerializer::Skip,
        }
    }

    fn encode_with_meta(
        &self,
        meta: Option<UiTransactionStatusMeta>,
    ) -> EncodedConfirmedTransactionWithStatusMeta {
        let transaction = VersionedTransaction::from(Transaction::new_unsigned(self.message()));
        let encoded = STANDARD.encode(
            bincode::serialize(&transaction).expect("serializing the transaction should succeed"),
        );
        EncodedConfirmedTransactionWithStatusMeta {
            slot: 0,
            transaction: EncodedTransactionWithStatusMeta {
                transaction: EncodedTransaction::Binary(encoded, TransactionBinaryEncoding::Base64),
                meta,
                version: None,
            },
            block_time: None,
        }
    }
}
