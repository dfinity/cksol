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
pub async fn credit_finalized_sweeps<R: CanisterRuntime>(runtime: &R) {
    let signatures = read_state(State::sweeps_awaiting_credit);
    if signatures.is_empty() {
        return;
    }

    let master_key = lazy_get_schnorr_master_key(runtime).await;
    let main_address = minter_address(&master_key, runtime);

    futures::future::join_all(
        signatures
            .into_iter()
            .take(MAX_CONCURRENT_RPC_CALLS)
            .map(async |signature| {
                credit_sweep(runtime, signature, &master_key, main_address).await
            }),
    )
    .await;
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
        Err(e) => {
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
fn amount_received_by(
    transaction: &EncodedConfirmedTransactionWithStatusMeta,
    main_address: Address,
    swept_addresses: &[(Address, Lamport)],
) -> Result<Lamport, SweepBalanceError> {
    let message = transaction
        .transaction
        .transaction
        .decode()
        .ok_or(SweepBalanceError::TransactionDecodingFailed)?
        .message;
    let meta = transaction
        .transaction
        .meta
        .as_ref()
        .ok_or(SweepBalanceError::NoMetaField)?;
    let balances = AccountBalances {
        account_keys: message.static_account_keys(),
        pre_balances: &meta.pre_balances,
        post_balances: &meta.post_balances,
    };

    let assumed_fee = FEE_PER_SIGNATURE * swept_addresses.len() as u64;
    for (address, sweepable_amount) in swept_addresses {
        let (index, pre, post) = balances.of(address)?;
        let expected_decrease = if index == FEE_PAYER_ACCOUNT_INDEX {
            (sweepable_amount + meta.fee)
                .checked_sub(assumed_fee)
                .ok_or(SweepBalanceError::UnexpectedFee { fee: meta.fee })?
        } else {
            *sweepable_amount
        };
        if pre.checked_sub(post) != Some(expected_decrease) {
            return Err(SweepBalanceError::UnexpectedBalanceChange {
                address: *address,
                pre,
                post,
                expected_decrease,
            });
        }
        if post < RENT_EXEMPTION_THRESHOLD {
            return Err(SweepBalanceError::NotRentExempt {
                address: *address,
                post,
            });
        }
    }

    let (_, pre, post) = balances.of(&main_address)?;
    post.checked_sub(pre)
        .ok_or(SweepBalanceError::MainAccountDebited { pre, post })
}

struct AccountBalances<'a> {
    account_keys: &'a [Address],
    pre_balances: &'a [Lamport],
    post_balances: &'a [Lamport],
}

impl AccountBalances<'_> {
    fn of(&self, address: &Address) -> Result<(usize, Lamport, Lamport), SweepBalanceError> {
        let index = self
            .account_keys
            .iter()
            .position(|key| key == address)
            .ok_or(SweepBalanceError::AddressNotInTransaction { address: *address })?;
        let pre = *self
            .pre_balances
            .get(index)
            .ok_or(SweepBalanceError::IncompleteBalances)?;
        let post = *self
            .post_balances
            .get(index)
            .ok_or(SweepBalanceError::IncompleteBalances)?;
        Ok((index, pre, post))
    }
}

#[derive(Debug, PartialEq, Eq, Error)]
enum SweepBalanceError {
    #[error("the sweep transaction could not be decoded")]
    TransactionDecodingFailed,
    #[error("the 'getTransaction' response has no 'meta' field")]
    NoMetaField,
    #[error("the balances in the metadata do not cover all account keys")]
    IncompleteBalances,
    #[error("the address {address} is not part of the sweep transaction")]
    AddressNotInTransaction { address: Address },
    #[error("the fee payer paid a fee of {fee} lamports that exceeds its transfer")]
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
}
