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
/// Panics if the nonce authority of the account is not the minter's main
/// address, since a wrong-authority account in the pool is a serious operator
/// error that must halt the minter loudly.
pub async fn read_verified_nonce<R: CanisterRuntime>(
    runtime: &R,
    account: Address,
    minter_address: Address,
) -> Result<Hash, GetNonceAccountError> {
    let nonce_account = get_nonce_account(runtime, account).await?;
    if nonce_account.authority != minter_address {
        panic!(
            "BUG: nonce account {account} has authority {} instead of the minter address {minter_address}",
            nonce_account.authority
        );
    }
    Ok(nonce_account.nonce)
}
