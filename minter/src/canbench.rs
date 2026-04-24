use crate::{
    lifecycle,
    numeric::{LedgerBurnIndex, LedgerMintIndex},
    runtime::IcCanisterRuntime,
    state::{
        audit::{process_event, replay_events},
        event::{
            DepositId, DepositSource, EventType, TransactionPurpose, VersionedMessage,
            WithdrawalRequest,
        },
        init_once_state, mutate_state, reset_state,
    },
    storage::{reset_events, total_event_count, with_event_iter},
};
use canbench_rs::bench;
use candid::Principal;
use cksol_types_internal::{Ed25519KeyName, InitArgs, SolanaNetwork};
use icrc_ledger_types::icrc1::account::Account;
use solana_signature::Signature;

const INDEX_OFFSET_QUARANTINE: usize = 10_000;
const INDEX_OFFSET_WITHDRAWAL: usize = 20_000;
const INDEX_OFFSET_FAILED: usize = 30_000;
const INDEX_OFFSET_EXPIRED: usize = 40_000;
const INDEX_OFFSET_RESUBMIT: usize = 50_000;

fn init_args() -> InitArgs {
    InitArgs {
        sol_rpc_canister_id: Principal::from_slice(&[1_u8; 20]),
        ledger_canister_id: Principal::from_slice(&[2_u8; 20]),
        manual_deposit_fee: 10_000,
        automated_deposit_fee: 10_000_000,
        master_key_name: Ed25519KeyName::default(),
        minimum_withdrawal_amount: 10_000_000,
        minimum_deposit_amount: 10_000_000,
        withdrawal_fee: 5_000_000,
        process_deposit_required_cycles: 1_000_000_000_000,
        solana_network: SolanaNetwork::Mainnet,
        deposit_consolidation_fee: 10_000_000_000,
    }
}

fn signature(i: usize) -> Signature {
    let mut bytes = [0u8; 64];
    bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    Signature::from(bytes)
}

fn principal(i: usize) -> Principal {
    let mut principal_bytes = [0u8; 29];
    principal_bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    Principal::from_slice(&principal_bytes)
}

fn deposit_id(i: usize) -> DepositId {
    DepositId {
        signature: signature(i),
        account: Account {
            owner: principal(i),
            subaccount: None,
        },
    }
}

fn message() -> solana_message::Message {
    let payer = solana_address::Address::from([0x42; 32]);
    solana_message::Message::new_with_blockhash(&[], Some(&payer), &solana_message::Hash::default())
}

fn minter_account() -> Account {
    Account {
        owner: Principal::from_slice(&[0xCA; 10]),
        subaccount: None,
    }
}

/// Populates the event log with ~10k events covering every event type
/// except Upgrade, then clears in-memory state so that
/// `replay_events` can rebuild it from stable storage.
fn setup_10k_events() {
    reset_events();
    reset_state();

    let runtime = IcCanisterRuntime::new();
    lifecycle::init(init_args(), runtime.clone());

    let deposit_fee: u64 = 10_000;
    let amount: u64 = 1_000_000_000;
    let withdrawal_fee: u64 = 5_000_000;
    let withdrawal_amount: u64 = 10_000_000;
    let minter = minter_account();

    // Successful deposit cycles: accept → mint → submit consolidation → succeed
    // 1000 × 4 = 4000 events
    for i in 0..1000 {
        let id = deposit_id(i);
        let sig = signature(i);
        let mint_index = LedgerMintIndex::from(i as u64);

        mutate_state(|s| {
            process_event(
                s,
                EventType::AcceptedDeposit {
                    deposit_id: id,
                    deposit_amount: amount,
                    amount_to_mint: amount - deposit_fee,
                    source: DepositSource::Manual,
                },
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(
                s,
                EventType::Minted {
                    deposit_id: id,
                    mint_block_index: mint_index,
                },
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(
                s,
                EventType::SubmittedTransaction {
                    signature: sig,
                    message: VersionedMessage::Legacy(message()),
                    signers: vec![minter],
                    slot: 0,
                    purpose: TransactionPurpose::ConsolidateDeposits {
                        mint_indices: vec![mint_index],
                    },
                },
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(
                s,
                EventType::SucceededTransaction { signature: sig },
                &runtime,
            )
        });
    }

    // Quarantined deposits: accept → quarantine
    // 200 × 2 = 400 events
    for i in 0..200 {
        let id = deposit_id(INDEX_OFFSET_QUARANTINE + i);

        mutate_state(|s| {
            process_event(
                s,
                EventType::AcceptedDeposit {
                    deposit_id: id,
                    deposit_amount: amount,
                    amount_to_mint: amount - deposit_fee,
                    source: DepositSource::Manual,
                },
                &runtime,
            )
        });
        mutate_state(|s| process_event(s, EventType::QuarantinedDeposit(id), &runtime));
    }

    // Withdrawal cycles: accept withdrawal → submit withdrawal → succeed
    // 500 × 3 = 1500 events
    for i in 0..500 {
        let sig = signature(INDEX_OFFSET_WITHDRAWAL + i);
        let burn_index = LedgerBurnIndex::from(i as u64);

        mutate_state(|s| {
            process_event(
                s,
                EventType::AcceptedWithdrawalRequest(WithdrawalRequest {
                    account: deposit_id(i).account,
                    solana_address: [0u8; 32],
                    burn_block_index: burn_index,
                    burned_amount: withdrawal_amount,
                    amount_to_transfer: withdrawal_amount - withdrawal_fee,
                }),
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(
                s,
                EventType::SubmittedTransaction {
                    signature: sig,
                    message: VersionedMessage::Legacy(message()),
                    signers: vec![minter],
                    slot: 0,
                    purpose: TransactionPurpose::WithdrawSol {
                        burn_indices: vec![burn_index],
                    },
                },
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(
                s,
                EventType::SucceededTransaction { signature: sig },
                &runtime,
            )
        });
    }

    // Failed consolidation cycles: accept → mint → submit → fail
    // 500 × 4 = 2000 events
    for i in 0..500 {
        let id = deposit_id(INDEX_OFFSET_FAILED + i);
        let sig = signature(INDEX_OFFSET_FAILED + i);
        let mint_index = LedgerMintIndex::from((INDEX_OFFSET_FAILED + i) as u64);

        mutate_state(|s| {
            process_event(
                s,
                EventType::AcceptedDeposit {
                    deposit_id: id,
                    deposit_amount: amount,
                    amount_to_mint: amount - deposit_fee,
                    source: DepositSource::Manual,
                },
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(
                s,
                EventType::Minted {
                    deposit_id: id,
                    mint_block_index: mint_index,
                },
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(
                s,
                EventType::SubmittedTransaction {
                    signature: sig,
                    message: VersionedMessage::Legacy(message()),
                    signers: vec![minter],
                    slot: 0,
                    purpose: TransactionPurpose::ConsolidateDeposits {
                        mint_indices: vec![mint_index],
                    },
                },
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(s, EventType::FailedTransaction { signature: sig }, &runtime)
        });
    }

    // Expired + resubmitted cycles: accept → mint → submit → expire → resubmit → succeed
    // 300 × 6 = 1800 events
    for i in 0..300 {
        let id = deposit_id(INDEX_OFFSET_EXPIRED + i);
        let old_sig = signature(INDEX_OFFSET_EXPIRED + i);
        let new_sig = signature(INDEX_OFFSET_RESUBMIT + i);
        let mint_index = LedgerMintIndex::from((INDEX_OFFSET_EXPIRED + i) as u64);

        mutate_state(|s| {
            process_event(
                s,
                EventType::AcceptedDeposit {
                    deposit_id: id,
                    deposit_amount: amount,
                    amount_to_mint: amount - deposit_fee,
                    source: DepositSource::Manual,
                },
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(
                s,
                EventType::Minted {
                    deposit_id: id,
                    mint_block_index: mint_index,
                },
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(
                s,
                EventType::SubmittedTransaction {
                    signature: old_sig,
                    message: VersionedMessage::Legacy(message()),
                    signers: vec![minter],
                    slot: 0,
                    purpose: TransactionPurpose::ConsolidateDeposits {
                        mint_indices: vec![mint_index],
                    },
                },
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(
                s,
                EventType::ExpiredTransaction { signature: old_sig },
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(
                s,
                EventType::ResubmittedTransaction {
                    old_signature: old_sig,
                    new_signature: new_sig,
                    new_slot: 1,
                },
                &runtime,
            )
        });
        mutate_state(|s| {
            process_event(
                s,
                EventType::SucceededTransaction { signature: new_sig },
                &runtime,
            )
        });
    }

    // Total: 1 (init) + 4000 + 400 + 1500 + 2000 + 1800 = 9701 events
    assert_eq!(total_event_count(), 9701);
    reset_state();
}

/// Measures the number of instructions to replay ~10k events during post_upgrade.
#[bench(raw)]
fn post_upgrade_10k_events() -> canbench_rs::BenchResult {
    setup_10k_events();

    canbench_rs::bench_fn(|| {
        init_once_state(with_event_iter(|events| replay_events(events)));
    })
}
