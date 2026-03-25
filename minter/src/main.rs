use candid::Principal;
use canlog::{Log, Sort};
use cksol_minter::consolidate::{DEPOSIT_CONSOLIDATION_DELAY, consolidate_deposits};
use cksol_minter::monitor::{MONITOR_SUBMITTED_TRANSACTIONS_DELAY, monitor_submitted_transactions};
use cksol_minter::withdraw_sol::{WITHDRAWAL_PROCESSING_DELAY, process_pending_withdrawals};
use cksol_minter::{
    address::lazy_get_schnorr_master_key, runtime::IcCanisterRuntime, state::read_state,
};
use cksol_types::{
    Address, DepositStatus, GetDepositAddressArgs, MinterInfo, UpdateBalanceArgs,
    UpdateBalanceError, WithdrawSolArgs, WithdrawSolError, WithdrawSolOk, WithdrawSolStatus,
};
use cksol_types_internal::{MinterArg, log::Priority};
use ic_http_types::{HttpRequest, HttpResponse, HttpResponseBuilder};
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
async fn withdraw_sol(args: WithdrawSolArgs) -> Result<WithdrawSolOk, WithdrawSolError> {
    let minimum_withdrawal_amount = read_state(|s| s.minimum_withdrawal_amount());
    if args.amount < minimum_withdrawal_amount {
        return Err(WithdrawSolError::AmountTooLow(minimum_withdrawal_amount));
    }

    let minter_account: Account = ic_cdk::api::canister_self().into();

    cksol_minter::withdraw_sol::withdraw_sol(
        &IcCanisterRuntime::new(),
        minter_account,
        ic_cdk::api::msg_caller(),
        args.from_subaccount,
        args.amount,
        args.address,
    )
    .await
}

#[ic_cdk::update]
fn withdraw_sol_status(block_index: u64) -> WithdrawSolStatus {
    cksol_minter::withdraw_sol::withdraw_sol_status(block_index)
}

#[ic_cdk::query]
fn get_events(
    args: cksol_types_internal::event::GetEventsArgs,
) -> cksol_types_internal::event::GetEventsResult {
    use cksol_minter::state::event::{Event, EventType};
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
            EventType::AcceptedWithdrawSolRequest(request) => {
                event::EventType::AcceptedWithdrawSolRequest {
                    account: request.account,
                    solana_address: request.solana_address,
                    burn_block_index: *request.burn_block_index.get(),
                    withdrawal_amount: request.withdrawal_amount,
                    withdrawal_fee: request.withdrawal_fee,
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
                transaction,
                signers,
                slot,
            } => event::EventType::SubmittedTransaction {
                signature: signature.into(),
                transaction: bincode::serialize(&transaction)
                    .expect("serializing transaction should succeed"),
                signers,
                slot,
            },
            EventType::ConsolidatedDeposits { mint_indices } => {
                event::EventType::ConsolidatedDeposits {
                    mint_indices: mint_indices.iter().map(|idx| *idx.get()).collect(),
                }
            }
            EventType::SentWithdrawalTransaction { transactions } => {
                event::EventType::SentWithdrawalTransaction {
                    transactions: transactions
                        .iter()
                        .map(|(idx, sig)| (*idx.get(), sig.into()))
                        .collect(),
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
        minimum_withdrawal_amount: s.minimum_withdrawal_amount(),
        minimum_deposit_amount: s.minimum_deposit_amount(),
        withdrawal_fee: s.withdrawal_fee(),
        update_balance_required_cycles: s.update_balance_required_cycles(),
    })
}

#[ic_cdk::query(hidden = true)]
fn http_request(request: HttpRequest) -> HttpResponse {
    match request.path() {
        "/metrics" => {
            todo!("DEFI-2670: add metrics")
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
    ic_cdk_timers::set_timer_interval(MONITOR_SUBMITTED_TRANSACTIONS_DELAY, async || {
        monitor_submitted_transactions(IcCanisterRuntime::new()).await;
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
        .join("cksol-minter.did");

    service_equal(
        CandidSource::Text(dbg!(&new_interface)),
        CandidSource::File(old_interface.as_path()),
    )
    .unwrap();
}
