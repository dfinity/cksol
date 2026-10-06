use crate::{
    address::minter_address,
    constants::RENT_EXEMPTION_THRESHOLD,
    lifecycle,
    numeric::{LedgerBurnIndex, LedgerMintIndex},
    rpc::BlockHeight,
    runtime::IcCanisterRuntime,
    state::{
        DepositBalance, QueuedDeposit, SchnorrPublicKey, Sweep,
        audit::{process_event, replay_events},
        event::{
            CreditedDeposit, EventType, Signer, TransactionPurpose, VersionedMessage,
            WithdrawalRequest,
        },
        init_once_state, mutate_state, reset_state,
    },
    storage::{reset_events, total_event_count, with_event_iter},
};
use canbench_rs::bench;
use candid::Principal;
use cksol_types_internal::{Ed25519KeyName, InitArgs, SolanaNetwork};
use ic_ed25519::{PocketIcMasterPublicKeyId, PublicKey};
use icrc_ledger_types::icrc1::account::Account;
use solana_signature::Signature;

const INDEX_OFFSET_QUARANTINE: usize = 10_000;
const INDEX_OFFSET_WITHDRAWAL: usize = 20_000;
const INDEX_OFFSET_DROPPED: usize = 30_000;
const INDEX_OFFSET_EXPIRED: usize = 40_000;
const INDEX_OFFSET_RESUBMIT: usize = 50_000;

fn init_args() -> InitArgs {
    InitArgs {
        sol_rpc_canister_id: Principal::from_slice(&[1_u8; 20]),
        ledger_canister_id: Principal::from_slice(&[2_u8; 20]),
        automated_deposit_fee: 10_000_000,
        master_key_name: Ed25519KeyName::default(),
        minimum_withdrawal_amount: 10_000_000,
        minimum_deposit_amount: 10_000_000,
        withdrawal_fee: 5_000_000,
        deposit_sol_required_cycles: 1_000_000_000_000,
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

fn account(i: usize) -> Account {
    Account {
        owner: principal(i),
        subaccount: None,
    }
}

/// The master key the generator records, as the management canister returns it
/// under PocketIC, so the sweep destination matches the one the state derives.
fn master_key() -> SchnorrPublicKey {
    SchnorrPublicKey {
        public_key: PublicKey::pocketic_key(PocketIcMasterPublicKeyId::Key1),
        chain_code: [1; 32],
    }
}

fn message() -> solana_message::Message {
    let payer = solana_address::Address::from([0x42; 32]);
    solana_message::Message::new_with_blockhash(&[], Some(&payer), &solana_message::Hash::default())
}

fn record(event: EventType) {
    let runtime = IcCanisterRuntime::new();
    mutate_state(|s| process_event(s, event, &runtime));
}

fn queue_and_sweep(deposit_id: u64, account_index: usize, amount: u64, sig: Signature) {
    let account = account(account_index);
    let deposit = QueuedDeposit {
        account,
        address: deposit_address(account_index),
        balance: DepositBalance::new(amount + RENT_EXEMPTION_THRESHOLD)
            .expect("BUG: the balance covers the rent exemption threshold"),
    };
    record(EventType::QueuedDeposit {
        deposit_id,
        account,
        address: deposit.address,
        balance: deposit.balance,
    });
    let sweep = Sweep::plan([(deposit_id, deposit)], minter_address(&master_key()));
    record(EventType::SubmittedTransaction {
        signature: sig,
        message: VersionedMessage::Legacy(sweep.sweep_message(solana_message::Hash::default())),
        signers: vec![Signer::Account(account)],
        purpose: TransactionPurpose::SweepDeposits {
            deposit_ids: vec![deposit_id],
        },
        block_height: BlockHeight::new(0),
    });
}

fn deposit_address(account_index: usize) -> solana_address::Address {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&(account_index as u64).to_le_bytes());
    solana_address::Address::from(bytes)
}

fn accept_and_submit_withdrawal(account_index: usize, burn_index: u64, sig: Signature) {
    const WITHDRAWAL_FEE: u64 = 5_000_000;
    const WITHDRAWAL_AMOUNT: u64 = 10_000_000;

    record(EventType::AcceptedWithdrawalRequest(WithdrawalRequest {
        account: account(account_index),
        solana_address: [0u8; 32],
        burn_block_index: LedgerBurnIndex::from(burn_index),
        burned_amount: WITHDRAWAL_AMOUNT,
        amount_to_transfer: WITHDRAWAL_AMOUNT - WITHDRAWAL_FEE,
    }));
    record(EventType::SubmittedTransaction {
        signature: sig,
        message: VersionedMessage::Legacy(message()),
        signers: vec![Signer::Minter],
        purpose: TransactionPurpose::WithdrawSol {
            burn_indices: vec![LedgerBurnIndex::from(burn_index)],
        },
        block_height: BlockHeight::new(0),
    });
}

/// Populates the event log with ~10k events covering every event type
/// except Upgrade and the quarantined-pending-mint case, then clears
/// in-memory state so that `replay_events` can rebuild it from stable storage.
fn setup_10k_events() {
    reset_events();
    reset_state();

    let runtime = IcCanisterRuntime::new();
    lifecycle::init(init_args(), runtime);

    // A sweep takes its destination from the recorded minter public key, so the
    // log starts with the event that records it.
    let master_key = master_key();
    record(EventType::MinterPublicKeyFetched {
        public_key: master_key.public_key,
        chain_code: master_key.chain_code,
    });

    const SWEEP_FEE: u64 = 5_000;
    let amount: u64 = 1_000_000_000;
    let mut next_deposit_id: u64 = 0;

    // Successful deposit cycles: queue → sweep → succeed → credit → mint
    // 1000 × 5 = 5000 events
    for i in 0..1000 {
        let deposit_id = next_deposit_id;
        next_deposit_id += 1;
        let sig = signature(i);

        queue_and_sweep(deposit_id, i, amount, sig);
        record(EventType::SucceededTransaction { signature: sig });
        record(EventType::CreditedSweep {
            signature: sig,
            amount_received: amount - SWEEP_FEE,
            mints: vec![CreditedDeposit {
                deposit_id,
                amount_to_mint: amount - SWEEP_FEE,
            }],
        });
        record(EventType::MintedSweptDeposit {
            deposit_id,
            mint_block_index: LedgerMintIndex::from(i as u64),
        });
    }

    // Quarantined sweeps: queue → sweep → succeed → quarantine
    // 200 × 4 = 800 events
    for i in 0..200 {
        let deposit_id = next_deposit_id;
        next_deposit_id += 1;
        let sig = signature(INDEX_OFFSET_QUARANTINE + i);

        queue_and_sweep(deposit_id, INDEX_OFFSET_QUARANTINE + i, amount, sig);
        record(EventType::SucceededTransaction { signature: sig });
        record(EventType::QuarantinedSweep { signature: sig });
    }

    // Withdrawal cycles: accept withdrawal → submit withdrawal → succeed
    // 500 × 3 = 1500 events
    for i in 0..500 {
        let sig = signature(INDEX_OFFSET_WITHDRAWAL + i);

        accept_and_submit_withdrawal(i, i as u64, sig);
        record(EventType::SucceededTransaction { signature: sig });
    }

    // Dropped sweeps: queue → sweep → fail
    // 500 × 3 = 1500 events
    for i in 0..500 {
        let deposit_id = next_deposit_id;
        next_deposit_id += 1;
        let sig = signature(INDEX_OFFSET_DROPPED + i);

        queue_and_sweep(deposit_id, INDEX_OFFSET_DROPPED + i, amount, sig);
        record(EventType::FailedTransaction { signature: sig });
    }

    // Expired + resubmitted withdrawal cycles: accept → submit → expire → resubmit → succeed
    // 300 × 5 = 1500 events
    for i in 0..300 {
        let old_sig = signature(INDEX_OFFSET_EXPIRED + i);
        let new_sig = signature(INDEX_OFFSET_RESUBMIT + i);

        accept_and_submit_withdrawal(
            INDEX_OFFSET_EXPIRED + i,
            (INDEX_OFFSET_EXPIRED + i) as u64,
            old_sig,
        );
        record(EventType::ExpiredTransaction { signature: old_sig });
        record(EventType::ResubmittedTransaction {
            old_signature: old_sig,
            new_signature: new_sig,
            new_block_height: BlockHeight::new(1),
        });
        record(EventType::SucceededTransaction { signature: new_sig });
    }

    // Total: 1 (init) + 1 (minter public key) + 5000 + 800 + 1500 + 1500 + 1500 = 10302 events
    assert_eq!(total_event_count(), 10302);
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
