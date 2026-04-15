use canbench_rs::bench;
use crate::{
    lifecycle,
    numeric::LedgerMintIndex,
    runtime::IcCanisterRuntime,
    state::{
        audit::{process_event, replay_events},
        event::{DepositId, EventType, TransactionPurpose, VersionedMessage},
        init_once_state, mutate_state, reset_state,
    },
    storage::{reset_events, with_event_iter},
};
use candid::Principal;
use cksol_types_internal::{Ed25519KeyName, InitArgs, SolanaNetwork};
use icrc_ledger_types::icrc1::account::Account;
use solana_signature::Signature;

const NUM_DEPOSIT_CYCLES: usize = 2500;

fn init_args() -> InitArgs {
    InitArgs {
        sol_rpc_canister_id: Principal::from_slice(&[1_u8; 20]),
        ledger_canister_id: Principal::from_slice(&[2_u8; 20]),
        deposit_fee: 10_000,
        master_key_name: Ed25519KeyName::default(),
        minimum_withdrawal_amount: 10_000_000,
        minimum_deposit_amount: 10_000_000,
        withdrawal_fee: 5_000_000,
        update_balance_required_cycles: 1_000_000_000_000,
        solana_network: SolanaNetwork::Mainnet,
        deposit_consolidation_fee: 10_000_000_000,
    }
}

fn signature(i: usize) -> Signature {
    let mut bytes = [0u8; 64];
    bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    Signature::from(bytes)
}

fn deposit_id(i: usize) -> DepositId {
    let mut principal_bytes = [0u8; 29];
    principal_bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    DepositId {
        signature: signature(i),
        account: Account {
            owner: Principal::from_slice(&principal_bytes),
            subaccount: None,
        },
    }
}

fn message() -> solana_message::Message {
    let payer = solana_address::Address::from([0x42; 32]);
    solana_message::Message::new_with_blockhash(
        &[],
        Some(&payer),
        &solana_message::Hash::default(),
    )
}

fn setup_10k_events() {
    reset_events();
    reset_state();

    let runtime = IcCanisterRuntime::new();
    lifecycle::init(init_args(), runtime.clone());

    let deposit_fee: u64 = 10_000;
    let amount: u64 = 1_000_000_000;

    for i in 0..NUM_DEPOSIT_CYCLES {
        let id = deposit_id(i);
        let sig = signature(i);
        let mint_index = LedgerMintIndex::from(i as u64);

        mutate_state(|s| {
            process_event(
                s,
                EventType::AcceptedManualDeposit {
                    deposit_id: id,
                    deposit_amount: amount,
                    amount_to_mint: amount - deposit_fee,
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
                    signers: vec![Account {
                        owner: Principal::from_slice(&[0xCA; 10]),
                        subaccount: None,
                    }],
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

    // Clear in-memory state but keep events in stable storage.
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
