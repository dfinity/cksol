//! Validation of the finalized transaction of an SPL withdrawal against its request.
use crate::{
    address::associated_token_address, sol_transfer::BATCH_WITHDRAWAL_TX_FEE,
    state::event::SplWithdrawalRequestTerms,
};
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_message::Message;
use solana_transaction_status_client_types::{
    UiTransactionStatusMeta, UiTransactionTokenBalance, option_serializer::OptionSerializer,
};
use thiserror::Error;

/// The validated outcome of the transaction of an SPL withdrawal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplWithdrawalOutcome {
    /// The tokens were transferred.
    Succeeded {
        /// The lamports that the main account spent: the transaction fee and the rent.
        lamports_spent: Lamport,
        /// The part of `lamports_spent` that created the destination token account.
        rent_spent: Lamport,
    },
    /// The transaction failed on-chain: no token moved and no account was created.
    Failed {
        /// The lamports that the main account spent on the transaction fee.
        lamports_spent: Lamport,
    },
}

/// Why the metadata of a finalized SPL withdrawal transaction cannot settle it. The
/// withdrawal stays in flight and its transaction is read again.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum SplWithdrawalSettlementError {
    #[error("the transaction metadata does not have a balance for each account")]
    IncompleteBalances,
    #[error("the transaction fee {actual} exceeds the expected fee {expected}")]
    UnexpectedFee { expected: Lamport, actual: Lamport },
    #[error("account {address} changed from {pre} to {post} lamports")]
    UnexpectedBalanceChange {
        address: Address,
        pre: Lamport,
        post: Lamport,
    },
    #[error("the message does not have the token account {0}")]
    MissingTokenAccount(Address),
    #[error("the transaction metadata does not have the token balance of {0}")]
    IncompleteTokenBalances(Address),
    #[error("the token balance of {0} names another mint, owner or token program")]
    UnexpectedTokenAccount(Address),
    #[error("the token balance of {address} changed from {pre} to {post}")]
    UnexpectedTokenBalanceChange {
        address: Address,
        pre: u64,
        post: u64,
    },
}

/// Validates the SOL and token balance changes of the finalized transaction, whose
/// message is the one that the minter signed for the request.
///
/// The rent is the lamports that the destination token account received, so a change of
/// the fee payer alone does not count as rent.
pub fn settle_spl_withdrawal(
    message: &Message,
    meta: &UiTransactionStatusMeta,
    terms: &SplWithdrawalRequestTerms,
) -> Result<SplWithdrawalOutcome, SplWithdrawalSettlementError> {
    use SplWithdrawalSettlementError::*;

    let keys = &message.account_keys;
    if meta.pre_balances.len() != keys.len() || meta.post_balances.len() != keys.len() {
        return Err(IncompleteBalances);
    }
    if meta.fee > BATCH_WITHDRAWAL_TX_FEE {
        return Err(UnexpectedFee {
            expected: BATCH_WITHDRAWAL_TX_FEE,
            actual: meta.fee,
        });
    }
    let index_of = |address: Address| {
        keys.iter()
            .position(|key| *key == address)
            .ok_or(MissingTokenAccount(address))
    };
    let treasury = associated_token_address(&keys[0], &terms.mint);
    let destination = associated_token_address(&terms.address, &terms.mint);
    let (treasury_index, destination_index) = (index_of(treasury)?, index_of(destination)?);
    let failed = meta.err.is_some();

    // A failed transaction creates no account, so it spends no rent.
    let rent_spent = if failed {
        0
    } else {
        meta.post_balances[destination_index].saturating_sub(meta.pre_balances[destination_index])
    };
    let lamports_spent = meta
        .fee
        .checked_add(rent_spent)
        .expect("BUG: SPL withdrawal cost overflows");
    // The fee payer pays the fee and the rent, and the destination token account
    // receives the rent. No other account changes its lamports.
    for (index, (pre, post)) in meta
        .pre_balances
        .iter()
        .zip(&meta.post_balances)
        .enumerate()
    {
        let expected_post = if index == 0 {
            pre.checked_sub(lamports_spent)
        } else if index == destination_index {
            pre.checked_add(rent_spent)
        } else {
            Some(*pre)
        };
        if Some(*post) != expected_post {
            return Err(UnexpectedBalanceChange {
                address: keys[index],
                pre: *pre,
                post: *post,
            });
        }
    }
    if failed {
        return Ok(SplWithdrawalOutcome::Failed { lamports_spent });
    }

    let transferred = terms
        .amount_to_transfer()
        .expect("BUG: an accepted SPL withdrawal transfers a positive amount");
    // The treasury exists before the transfer. A new destination has no balance before.
    let (minter_address, mint) = (keys[0], terms.mint);
    let treasury_pre = token_amount(
        &meta.pre_token_balances,
        treasury_index,
        treasury,
        minter_address,
        mint,
    )?
    .ok_or(IncompleteTokenBalances(treasury))?;
    let treasury_post = token_amount(
        &meta.post_token_balances,
        treasury_index,
        treasury,
        minter_address,
        mint,
    )?
    .ok_or(IncompleteTokenBalances(treasury))?;
    let destination_pre = token_amount(
        &meta.pre_token_balances,
        destination_index,
        destination,
        terms.address,
        mint,
    )?
    .unwrap_or(0);
    let destination_post = token_amount(
        &meta.post_token_balances,
        destination_index,
        destination,
        terms.address,
        mint,
    )?
    .ok_or(IncompleteTokenBalances(destination))?;
    if treasury_pre.checked_sub(treasury_post) != Some(transferred) {
        return Err(UnexpectedTokenBalanceChange {
            address: treasury,
            pre: treasury_pre,
            post: treasury_post,
        });
    }
    if destination_post.checked_sub(destination_pre) != Some(transferred) {
        return Err(UnexpectedTokenBalanceChange {
            address: destination,
            pre: destination_pre,
            post: destination_post,
        });
    }
    Ok(SplWithdrawalOutcome::Succeeded {
        lamports_spent,
        rent_spent,
    })
}

/// The token amount that the metadata reports for the account at the given index, or
/// `None` if it reports no balance for it. The entry must name the expected mint, owner
/// and token program.
fn token_amount(
    balances: &OptionSerializer<Vec<UiTransactionTokenBalance>>,
    account_index: usize,
    address: Address,
    owner: Address,
    mint: Address,
) -> Result<Option<u64>, SplWithdrawalSettlementError> {
    use SplWithdrawalSettlementError::*;

    let OptionSerializer::Some(balances) = balances else {
        return Ok(None);
    };
    let Some(balance) = balances
        .iter()
        .find(|balance| usize::from(balance.account_index) == account_index)
    else {
        return Ok(None);
    };
    let (OptionSerializer::Some(actual_owner), OptionSerializer::Some(program)) =
        (&balance.owner, &balance.program_id)
    else {
        return Err(IncompleteTokenBalances(address));
    };
    if balance.mint != mint.to_string()
        || *actual_owner != owner.to_string()
        || *program != spl_token_interface::id().to_string()
    {
        return Err(UnexpectedTokenAccount(address));
    }
    balance
        .ui_token_amount
        .amount
        .parse()
        .map(Some)
        .map_err(|_| IncompleteTokenBalances(address))
}
