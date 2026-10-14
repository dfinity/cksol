use crate::{
    address::{account_address, minter_public_key},
    runtime::CanisterRuntime,
    state::{InFlightDeposit, State, WatchedAddress, WatchlistFull, mutate_state},
    utils::assert_valid_deposit_owner,
};
use cksol_types::{WatchDepositAddressError, WatchDepositAddressOk, WatchDepositAddressStatus};
use icrc_ledger_types::icrc1::account::Account;
use solana_address::Address;

#[cfg(test)]
mod tests;

pub fn watch_deposit_address<R: CanisterRuntime>(
    runtime: &R,
    account: Account,
) -> Result<WatchDepositAddressOk, WatchDepositAddressError> {
    assert_valid_deposit_owner(&account, runtime.canister_self());
    let master_key = minter_public_key()
        .map_err(|e| WatchDepositAddressError::TemporarilyUnavailable(e.to_string()))?;
    let address = account_address(&master_key, &account);
    let now = runtime.time();

    mutate_state(|state| {
        let automatic_deposit_fee = state.automatic_deposit_fee().ok_or_else(|| {
            WatchDepositAddressError::TemporarilyUnavailable(
                "the automatic deposit fee is not configured".to_string(),
            )
        })?;
        let status = status_after_registration(state, now, account, address)?;
        Ok(WatchDepositAddressOk {
            address: address.into(),
            minimum_deposit_amount: state.minimum_deposit_amount(),
            automatic_deposit_fee,
            status,
            latest_deposit_id: state.deposits().latest_id(&account),
        })
    })
}

fn status_after_registration(
    state: &mut State,
    now: u64,
    account: Account,
    address: Address,
) -> Result<WatchDepositAddressStatus, WatchDepositAddressError> {
    match state.deposits().in_flight(&account) {
        Some(InFlightDeposit::Processing(deposit_id)) => {
            return Ok(WatchDepositAddressStatus::Detected { deposit_id });
        }
        Some(InFlightDeposit::Quarantined(deposit_id)) => {
            return Err(WatchDepositAddressError::Quarantined { deposit_id });
        }
        None => {}
    }
    state
        .watchlist_mut()
        .register(now, account, address)
        .map(watching)
        .map_err(|WatchlistFull| WatchDepositAddressError::TooManyWatchedAddresses)
}

fn watching(watched: WatchedAddress) -> WatchDepositAddressStatus {
    WatchDepositAddressStatus::Watching {
        valid_until: watched.valid_until,
        scan_count: watched.scan_count,
        last_scanned_at: watched.last_scanned_at,
        next_scan_at: watched.next_scan_at,
    }
}
