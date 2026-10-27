use crate::{
    address::{account_address, minter_public_key},
    runtime::CanisterRuntime,
    state::{InFlightDeposit, WatchedAddress, WatchlistFull, mutate_state},
    utils::{assert_valid_deposit_owner, timed_sized_map::Timestamp},
};
use cksol_types::{WatchDepositAddressError, WatchDepositAddressOk, WatchDepositAddressStatus};
use icrc_ledger_types::icrc1::account::Account;

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
    let now = Timestamp::from_nanos(runtime.time());

    mutate_state(|state| {
        let automatic_deposit_fee = state.automatic_deposit_fee().ok_or_else(|| {
            WatchDepositAddressError::TemporarilyUnavailable(
                "the automatic deposit fee is not configured".to_string(),
            )
        })?;
        let latest_deposit_id = state.deposits().latest_id(&account);
        let status = match state.deposits().in_flight(&account) {
            Some(InFlightDeposit::Processing(deposit_id)) => {
                WatchDepositAddressStatus::Detected { deposit_id }
            }
            Some(InFlightDeposit::Quarantined(deposit_id)) => {
                return Err(WatchDepositAddressError::Quarantined { deposit_id });
            }
            None => state
                .watchlist_mut()
                .register(now, account, address)
                .map_err(
                    |WatchlistFull| WatchDepositAddressError::TooManyWatchedAddresses {
                        latest_deposit_id,
                    },
                )?
                .into(),
        };
        Ok(WatchDepositAddressOk {
            address: address.into(),
            minimum_deposit_amount: state.minimum_deposit_amount(),
            automatic_deposit_fee,
            status,
            latest_deposit_id,
        })
    })
}

impl From<WatchedAddress> for WatchDepositAddressStatus {
    fn from(watched: WatchedAddress) -> Self {
        WatchDepositAddressStatus::Watching {
            valid_until: watched.valid_until.as_nanos(),
            scan_count: watched.scan_count,
            last_scanned_at: watched.last_scanned_at.map(Timestamp::as_nanos),
            next_scan_at: watched.next_scan_at.map(Timestamp::as_nanos),
        }
    }
}
