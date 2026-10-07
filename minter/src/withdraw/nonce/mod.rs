use crate::{
    rpc::{GetNonceAccountError, get_nonce_account},
    runtime::CanisterRuntime,
};
use solana_address::Address;
use solana_hash::Hash;
use thiserror::Error;

#[cfg(test)]
mod tests;

/// Reads the given durable nonce account and returns the nonce value it stores,
/// provided that the account is an initialized nonce account whose nonce
/// authority is the minter's main address.
pub async fn read_verified_nonce<R: CanisterRuntime>(
    runtime: &R,
    account: Address,
    minter_address: Address,
) -> Result<Hash, ReadNonceError> {
    let nonce_account = get_nonce_account(runtime, account).await?;
    if nonce_account.authority != minter_address {
        return Err(ReadNonceError::ForeignAuthority {
            authority: nonce_account.authority,
            minter_address,
        });
    }
    Ok(nonce_account.nonce)
}

#[derive(Debug, PartialEq, Error)]
pub enum ReadNonceError {
    #[error(transparent)]
    GetNonceAccount(#[from] GetNonceAccountError),
    #[error("Nonce authority {authority} instead of the minter address {minter_address}")]
    ForeignAuthority {
        authority: Address,
        minter_address: Address,
    },
}
