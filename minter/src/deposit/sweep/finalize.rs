use crate::{
    address::{account_address, lazy_get_schnorr_master_key, minter_address},
    constants::{FEE_PER_SIGNATURE, MAX_CONCURRENT_RPC_CALLS, RENT_EXEMPTION_THRESHOLD},
    rpc::get_transaction,
    runtime::CanisterRuntime,
    state::{
        SchnorrPublicKey, State, audit::process_event, event::EventType, mutate_state, read_state,
    },
};
use canlog::log;
use cksol_types_internal::log::Priority;
use derive_more::From;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_signature::Signature;
use solana_transaction_status_client_types::EncodedConfirmedTransactionWithStatusMeta;
use thiserror::Error;

#[cfg(test)]
mod tests;

/// The fee payer of a Solana transaction is the first of its account keys.
const FEE_PAYER_ACCOUNT_INDEX: usize = 0;

/// Credits the deposits of the sweep transactions that have been finalized successfully.
///
/// The amount received is read from the transaction metadata rather than derived from
/// the fee schedule, so that the ckSOL supply is covered even if Solana charges less
/// than the fee the sweep was built with. A sweep whose metadata does not match the
/// minter's model of the transaction is quarantined instead of credited.
///
/// Returns whether the finalization timer must run again immediately.
pub async fn credit_finalized_sweeps<R: CanisterRuntime>(runtime: &R) -> bool {
    let signatures = read_state(State::sweeps_awaiting_credit);
    if signatures.is_empty() {
        return false;
    }

    let master_key = lazy_get_schnorr_master_key(runtime).await;
    let main_address = minter_address(&master_key, runtime);

    futures::future::join_all(
        signatures
            .iter()
            .take(MAX_CONCURRENT_RPC_CALLS)
            .map(async |signature| {
                credit_sweep(runtime, *signature, &master_key, main_address).await
            }),
    )
    .await;

    signatures.len() > MAX_CONCURRENT_RPC_CALLS
}

async fn credit_sweep<R: CanisterRuntime>(
    runtime: &R,
    signature: Signature,
    master_key: &SchnorrPublicKey,
    main_address: Address,
) {
    let transaction = match get_transaction(runtime, signature).await {
        Ok(Some(transaction)) => transaction,
        Ok(None) => {
            log!(
                Priority::Info,
                "Finalized sweep {signature} was not returned by getTransaction, retrying later"
            );
            return;
        }
        Err(e) => {
            log!(
                Priority::Info,
                "Failed to fetch finalized sweep {signature}: {e}, retrying later"
            );
            return;
        }
    };

    let swept_addresses: Vec<(Address, Lamport)> =
        read_state(|state| state.finalized_deposits_of(&signature))
            .into_iter()
            .map(|(_, deposit)| {
                (
                    account_address(master_key, &deposit.account),
                    deposit.sweepable_amount,
                )
            })
            .collect();

    let event = match amount_received_by(&transaction, main_address, &swept_addresses) {
        Ok(amount_received) => EventType::CreditedSweep {
            signature,
            amount_received,
        },
        Err(SweepMetadataError::Unreadable(e)) => {
            log!(
                Priority::Info,
                "Could not read the metadata of sweep {signature}: {e}, retrying later"
            );
            return;
        }
        Err(SweepMetadataError::Mismatch(e)) => {
            log!(
                Priority::Error,
                "Quarantining the deposits of sweep {signature}: {e}"
            );
            EventType::QuarantinedSweep { signature }
        }
    };
    mutate_state(|state| process_event(state, event, runtime));
}

/// Returns the increase of the main account balance reported by the transaction metadata,
/// after checking that every swept deposit address decreased by exactly the amount the
/// sweep transfers from it, plus the transaction fee for the fee payer, and is left
/// rent-exempt. A transfer that arrived after the balance check legitimately leaves more
/// than the rent exemption threshold on a deposit address.
///
/// The main account must have received all but at most the assumed transaction fee of the
/// swept amount, which bounds the shortfall that the deposits of the sweep have to bear.
fn amount_received_by(
    transaction: &EncodedConfirmedTransactionWithStatusMeta,
    main_address: Address,
    swept_addresses: &[(Address, Lamport)],
) -> Result<Lamport, SweepMetadataError> {
    let message = transaction
        .transaction
        .transaction
        .decode()
        .ok_or(UnreadableMetadata::TransactionDecodingFailed)?
        .message;
    let meta = transaction
        .transaction
        .meta
        .as_ref()
        .ok_or(UnreadableMetadata::NoMetaField)?;
    let balances = AccountBalances {
        account_keys: message.static_account_keys(),
        pre_balances: &meta.pre_balances,
        post_balances: &meta.post_balances,
    };

    let assumed_fee = FEE_PER_SIGNATURE * swept_addresses.len() as u64;
    let mut swept_amount: Lamport = 0;
    for (address, sweepable_amount) in swept_addresses {
        let (index, pre, post) = balances.of(address)?;
        let expected_decrease = if index == FEE_PAYER_ACCOUNT_INDEX {
            sweepable_amount
                .checked_add(meta.fee)
                .and_then(|paid| paid.checked_sub(assumed_fee))
                .ok_or(SweepMismatch::UnexpectedFee { fee: meta.fee })?
        } else {
            *sweepable_amount
        };
        if pre.checked_sub(post) != Some(expected_decrease) {
            return Err(SweepMismatch::UnexpectedBalanceChange {
                address: *address,
                pre,
                post,
                expected_decrease,
            }
            .into());
        }
        if post < RENT_EXEMPTION_THRESHOLD {
            return Err(SweepMismatch::NotRentExempt {
                address: *address,
                post,
            }
            .into());
        }
        swept_amount += sweepable_amount;
    }

    let (_, pre, post) = balances.of(&main_address)?;
    let amount_received = post
        .checked_sub(pre)
        .ok_or(SweepMismatch::MainAccountDebited { pre, post })?;
    if swept_amount.saturating_sub(amount_received) > assumed_fee {
        return Err(SweepMismatch::AmountReceivedTooSmall {
            swept_amount,
            amount_received,
            assumed_fee,
        }
        .into());
    }
    Ok(amount_received)
}

struct AccountBalances<'a> {
    account_keys: &'a [Address],
    pre_balances: &'a [Lamport],
    post_balances: &'a [Lamport],
}

impl AccountBalances<'_> {
    fn of(&self, address: &Address) -> Result<(usize, Lamport, Lamport), SweepMetadataError> {
        let index = self
            .account_keys
            .iter()
            .position(|key| key == address)
            .ok_or(SweepMismatch::AddressNotInTransaction { address: *address })?;
        let pre = *self
            .pre_balances
            .get(index)
            .ok_or(UnreadableMetadata::IncompleteBalances)?;
        let post = *self
            .post_balances
            .get(index)
            .ok_or(UnreadableMetadata::IncompleteBalances)?;
        Ok((index, pre, post))
    }
}

#[derive(Debug, PartialEq, Eq, Error, From)]
enum SweepMetadataError {
    /// The response says nothing about the sweep, so fetching it again may succeed.
    #[error("{0}")]
    Unreadable(UnreadableMetadata),
    /// The metadata contradicts the minter's model of the sweep.
    #[error("{0}")]
    Mismatch(SweepMismatch),
}

#[derive(Debug, PartialEq, Eq, Error)]
enum UnreadableMetadata {
    #[error("the sweep transaction could not be decoded")]
    TransactionDecodingFailed,
    #[error("the 'getTransaction' response has no 'meta' field")]
    NoMetaField,
    #[error("the balances in the metadata do not cover all account keys")]
    IncompleteBalances,
}

#[derive(Debug, PartialEq, Eq, Error)]
enum SweepMismatch {
    #[error("the address {address} is not part of the sweep transaction")]
    AddressNotInTransaction { address: Address },
    #[error("the fee payer paid a fee of {fee} lamports that does not fit its transfer")]
    UnexpectedFee { fee: Lamport },
    #[error(
        "the address {address} went from {pre} to {post} lamports instead of decreasing by {expected_decrease}"
    )]
    UnexpectedBalanceChange {
        address: Address,
        pre: Lamport,
        post: Lamport,
        expected_decrease: Lamport,
    },
    #[error(
        "the address {address} is left with {post} lamports, below the rent exemption threshold"
    )]
    NotRentExempt { address: Address, post: Lamport },
    #[error("the main account went from {pre} to {post} lamports")]
    MainAccountDebited { pre: Lamport, post: Lamport },
    #[error(
        "the main account received {amount_received} of the {swept_amount} lamports swept, more than the assumed fee of {assumed_fee} lamports short"
    )]
    AmountReceivedTooSmall {
        swept_amount: Lamport,
        amount_received: Lamport,
        assumed_fee: Lamport,
    },
}
