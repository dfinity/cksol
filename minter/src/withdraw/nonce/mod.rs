use crate::{
    rpc::{GetNonceAccountError, get_nonce_account},
    runtime::CanisterRuntime,
};
use solana_address::Address;
use solana_hash::Hash;

#[cfg(test)]
mod tests;

/// Reads the given durable nonce account and returns the nonce value it stores.
///
/// # Panics
/// Panics if the account is not owned by the system program or if its nonce
/// authority is not the minter's main address, since such an account in the
/// pool is a serious operator error that must halt the minter loudly.
pub async fn read_verified_nonce<R: CanisterRuntime>(
    runtime: &R,
    account: Address,
    minter_address: Address,
) -> Result<Hash, GetNonceAccountError> {
    let nonce_account = match get_nonce_account(runtime, account).await {
        Ok(nonce_account) => nonce_account,
        Err(GetNonceAccountError::NotOwnedBySystemProgram { owner, executable }) => panic!(
            "BUG: nonce account {account} is owned by {owner} (executable: {executable}) instead of the system program"
        ),
        Err(error) => return Err(error),
    };
    if nonce_account.authority != minter_address {
        panic!(
            "BUG: nonce account {account} has authority {} instead of the minter address {minter_address}",
            nonce_account.authority
        );
    }
    Ok(nonce_account.nonce)
}
