use candid::Principal;
use canlog::{Log, Sort};
use cksol_minter::withdraw::{WITHDRAWAL_PROCESSING_DELAY, process_pending_withdrawals};
use cksol_minter::{
    address::lazy_get_schnorr_master_key, runtime::IcCanisterRuntime, state::read_state,
};
use cksol_minter::{
    consolidate::{DEPOSIT_CONSOLIDATION_DELAY, consolidate_deposits},
    monitor::{
        FINALIZE_TRANSACTIONS_DELAY, RESUBMIT_TRANSACTIONS_DELAY, finalize_transactions,
        resubmit_transactions,
    },
};
use cksol_types::{
    Address, DepositStatus, GetDepositAddressArgs, MinterInfo, UpdateBalanceArgs,
    UpdateBalanceError, WithdrawalArgs, WithdrawalError, WithdrawalOk, WithdrawalStatus,
};
use cksol_types_internal::{MinterArg, log::Priority};
use ic_http_types::{HttpRequest, HttpResponse, HttpResponseBuilder};
use ic_metrics_encoder::MetricsEncoder;
use icrc_ledger_types::icrc1::account::{Account, Subaccount};
use std::{str::FromStr, time::Duration};

#[ic_cdk::init]
fn init(args: MinterArg) {
    match args {
        MinterArg::Init(init) => {
            cksol_minter::lifecycle::init(init, IcCanisterRuntime::new());
        }
        MinterArg::Upgrade(_) => {
            ic_cdk::trap("cannot init canister state with upgrade args");
        }
    }
    setup_timers();
}

#[ic_cdk::post_upgrade]
fn post_upgrade(args: Option<MinterArg>) {
    match args {
        Some(MinterArg::Init(_)) => {
            ic_cdk::trap("cannot upgrade canister state with init args");
        }
        Some(MinterArg::Upgrade(args)) => {
            cksol_minter::lifecycle::post_upgrade(Some(args), IcCanisterRuntime::new());
        }
        None => {
            cksol_minter::lifecycle::post_upgrade(None, IcCanisterRuntime::new());
        }
    }
    setup_timers();
}

#[ic_cdk::update]
async fn get_deposit_address(args: GetDepositAddressArgs) -> Address {
    let account = assert_non_anonymous_account(args.owner, args.subaccount);
    cksol_minter::address::get_deposit_address(account)
        .await
        .into()
}

#[ic_cdk::update]
async fn update_balance(args: UpdateBalanceArgs) -> Result<DepositStatus, UpdateBalanceError> {
    let account = assert_non_anonymous_account(args.owner, args.subaccount);
    cksol_minter::update_balance::update_balance(
        IcCanisterRuntime::new(),
        account,
        args.signature.into(),
    )
    .await
}

#[ic_cdk::update]
async fn withdraw(args: WithdrawalArgs) -> Result<WithdrawalOk, WithdrawalError> {
    let account = assert_non_anonymous_account(None, args.from_subaccount);

    cksol_minter::withdraw::withdraw(
        &IcCanisterRuntime::new(),
        account,
        args.amount,
        args.address,
    )
    .await
}

#[ic_cdk::update]
fn withdrawal_status(block_index: u64) -> WithdrawalStatus {
    cksol_minter::withdraw::withdrawal_status(block_index)
}

#[ic_cdk::query]
fn get_events(
    args: cksol_types_internal::event::GetEventsArgs,
) -> cksol_types_internal::event::GetEventsResult {
    use cksol_minter::state::event::{Event, EventType, TransactionPurpose, VersionedMessage};
    use cksol_types_internal::event;

    const MAX_EVENTS_PER_RESPONSE: u64 = 2_000;

    fn map_event(event: Event) -> event::Event {
        event::Event {
            timestamp: event.timestamp,
            payload: map_event_type(event.payload),
        }
    }

    fn map_event_type(event_type: EventType) -> event::EventType {
        match event_type {
            EventType::Init(args) => event::EventType::Init(args),
            EventType::Upgrade(args) => event::EventType::Upgrade(args),
            EventType::AcceptedWithdrawalRequest(request) => {
                event::EventType::AcceptedWithdrawalRequest {
                    account: request.account,
                    solana_address: request.solana_address,
                    burn_block_index: *request.burn_block_index.get(),
                    amount_to_burn: request.amount_to_burn,
                    withdrawal_amount: request.withdrawal_amount,
                }
            }
            EventType::AcceptedDeposit {
                deposit_id,
                deposit_amount,
                amount_to_mint,
            } => event::EventType::AcceptedDeposit {
                signature: deposit_id.signature.into(),
                account: deposit_id.account,
                deposit_amount,
                amount_to_mint,
            },
            EventType::Minted {
                deposit_id,
                mint_block_index,
            } => event::EventType::Minted {
                signature: deposit_id.signature.into(),
                account: deposit_id.account,
                mint_block_index: *mint_block_index.get(),
            },
            EventType::QuarantinedDeposit(deposit_id) => event::EventType::QuarantinedDeposit {
                signature: deposit_id.signature.into(),
                account: deposit_id.account,
            },
            EventType::SubmittedTransaction {
                signature,
                message,
                signers,
                slot,
                purpose,
            } => {
                let purpose = match purpose {
                    TransactionPurpose::ConsolidateDeposits { mint_indices } => {
                        event::TransactionPurpose::ConsolidateDeposits {
                            mint_indices: mint_indices.iter().map(|idx| *idx.get()).collect(),
                        }
                    }
                    TransactionPurpose::WithdrawSol { burn_indices } => {
                        event::TransactionPurpose::WithdrawSol {
                            burn_indices: burn_indices.iter().map(|idx| *idx.get()).collect(),
                        }
                    }
                };
                event::EventType::SubmittedTransaction {
                    signature: signature.into(),
                    transaction: match message {
                        VersionedMessage::Legacy(message) => {
                            event::VersionedTransactionMessage::Legacy(
                                bincode::serialize(&message)
                                    .expect("serializing transaction should succeed"),
                            )
                        }
                    },
                    signers,
                    slot,
                    purpose,
                }
            }
            EventType::ResubmittedTransaction {
                old_signature,
                new_signature,
                new_slot,
            } => event::EventType::ResubmittedTransaction {
                old_signature: old_signature.into(),
                new_signature: new_signature.into(),
                new_slot,
            },
            EventType::SucceededTransaction { signature } => {
                event::EventType::SucceededTransaction {
                    signature: signature.into(),
                }
            }
            EventType::FailedTransaction { signature } => event::EventType::FailedTransaction {
                signature: signature.into(),
            },
            EventType::ExpiredTransaction { signature } => event::EventType::ExpiredTransaction {
                signature: signature.into(),
            },
        }
    }

    let events = cksol_minter::storage::with_event_iter(|it| {
        it.skip(args.start as usize)
            .take(args.length.min(MAX_EVENTS_PER_RESPONSE) as usize)
            .map(map_event)
            .collect()
    });
    event::GetEventsResult {
        events,
        total_event_count: cksol_minter::storage::total_event_count(),
    }
}

#[ic_cdk::query]
fn get_minter_info() -> MinterInfo {
    read_state(|s| MinterInfo {
        deposit_fee: s.deposit_fee(),
        deposit_consolidation_fee: s.deposit_consolidation_fee(),
        minimum_withdrawal_amount: s.minimum_withdrawal_amount(),
        minimum_deposit_amount: s.minimum_deposit_amount(),
        withdrawal_fee: s.withdrawal_fee(),
        update_balance_required_cycles: s.update_balance_required_cycles(),
        balance: s.balance(),
    })
}

#[ic_cdk::query(hidden = true)]
fn http_request(request: HttpRequest) -> HttpResponse {
    match request.path() {
        "/dashboard" => {
            use askama::Template;
            use cksol_minter::dashboard::{DashboardPaginationParameters, DashboardTemplate};
            let pagination = match DashboardPaginationParameters::from_query_params(&request) {
                Ok(p) => p,
                Err(e) => {
                    return HttpResponseBuilder::bad_request()
                        .with_body_and_content_length(e)
                        .build();
                }
            };
            let runtime = IcCanisterRuntime::new();
            let dashboard =
                read_state(|state| DashboardTemplate::from_state(state, &runtime, pagination));
            HttpResponseBuilder::ok()
                .header("Content-Type", "text/html; charset=utf-8")
                .with_body_and_content_length(dashboard.render().unwrap())
                .build()
        }
        "/metrics" => {
            let mut writer = MetricsEncoder::new(vec![], ic_cdk::api::time() as i64 / 1_000_000);

            match read_state(|s| cksol_minter::metrics::encode_metrics(&mut writer, s)) {
                Ok(()) => HttpResponseBuilder::ok()
                    .header("Content-Type", "text/plain; version=0.0.4")
                    .header("Cache-Control", "no-store")
                    .with_body_and_content_length(writer.into_inner())
                    .build(),
                Err(err) => {
                    HttpResponseBuilder::server_error(format!("Failed to encode metrics: {err}"))
                        .build()
                }
            }
        }
        "/logs" => {
            let max_skip_timestamp = match request.raw_query_param("time") {
                Some(arg) => match u64::from_str(arg) {
                    Ok(value) => value,
                    Err(_) => {
                        return HttpResponseBuilder::bad_request()
                            .with_body_and_content_length("failed to parse the 'time' parameter")
                            .build();
                    }
                },
                None => 0,
            };

            let mut log: Log<Priority> = Default::default();

            match request.raw_query_param("priority").map(Priority::from_str) {
                Some(Ok(priority)) => match priority {
                    Priority::Error => log.push_logs(Priority::Error),
                    Priority::Info => log.push_logs(Priority::Info),
                    Priority::Debug => log.push_logs(Priority::Debug),
                },
                Some(Err(_)) | None => {
                    log.push_logs(Priority::Error);
                    log.push_logs(Priority::Info);
                    log.push_logs(Priority::Debug);
                }
            }

            log.entries
                .retain(|entry| entry.timestamp >= max_skip_timestamp);

            fn ordering_from_query_params(sort: Option<&str>, max_skip_timestamp: u64) -> Sort {
                match sort.map(Sort::from_str) {
                    Some(Ok(order)) => order,
                    Some(Err(_)) | None => {
                        if max_skip_timestamp == 0 {
                            Sort::Ascending
                        } else {
                            Sort::Descending
                        }
                    }
                }
            }

            log.sort_logs(ordering_from_query_params(
                request.raw_query_param("sort"),
                max_skip_timestamp,
            ));

            const MAX_BODY_SIZE: usize = 2_000_000;
            HttpResponseBuilder::ok()
                .header("Content-Type", "application/json; charset=utf-8")
                .with_body_and_content_length(log.serialize_logs(MAX_BODY_SIZE))
                .build()
        }
        _ => HttpResponseBuilder::not_found().build(),
    }
}

fn assert_non_anonymous_account(
    owner: Option<Principal>,
    subaccount: Option<Subaccount>,
) -> Account {
    let owner = owner.unwrap_or_else(ic_cdk::api::msg_caller);
    assert_ne!(
        owner,
        Principal::anonymous(),
        "the owner must be non-anonymous"
    );
    Account { owner, subaccount }
}

fn setup_timers() {
    ic_cdk_timers::set_timer(Duration::from_secs(0), async {
        // Initialize the minter's Ed25519 public key
        let _ = lazy_get_schnorr_master_key().await;
    });
    ic_cdk_timers::set_timer_interval(DEPOSIT_CONSOLIDATION_DELAY, async || {
        consolidate_deposits(IcCanisterRuntime::new()).await;
    });
    ic_cdk_timers::set_timer_interval(WITHDRAWAL_PROCESSING_DELAY, async || {
        process_pending_withdrawals(&IcCanisterRuntime::new()).await;
    });
    ic_cdk_timers::set_timer_interval(FINALIZE_TRANSACTIONS_DELAY, async || {
        finalize_transactions(IcCanisterRuntime::new()).await;
    });
    ic_cdk_timers::set_timer_interval(RESUBMIT_TRANSACTIONS_DELAY, async || {
        resubmit_transactions(IcCanisterRuntime::new()).await;
    });
}

fn main() {}

#[test]
fn check_candid_interface_compatibility() {
    use candid_parser::utils::{CandidSource, service_equal};

    candid::export_service!();

    let new_interface = __export_service();

    // check the public interface against the actual one
    let old_interface = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("cksol_minter.did");

    service_equal(
        CandidSource::Text(dbg!(&new_interface)),
        CandidSource::File(old_interface.as_path()),
    )
    .unwrap();
}
