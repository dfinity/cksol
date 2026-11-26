use super::*;
use crate::{
    spl_transfer::{build_spl_withdrawal_message, withdrawal::SplWithdrawalTransfer},
    state::SupportedSplToken,
};
use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use solana_hash::Hash;

const FEE: Lamport = 5_000;
const RENT: Lamport = 2_039_280;
const AMOUNT: u64 = 1_000;
const TOKEN_FEE: u64 = 25;
const TRANSFERRED: u64 = AMOUNT - TOKEN_FEE;

fn minter() -> Address {
    [1; 32].into()
}

fn mint() -> Address {
    [2; 32].into()
}

fn terms() -> SplWithdrawalRequestTerms {
    SplWithdrawalRequestTerms {
        account: Account {
            owner: Principal::from_slice(&[9]),
            subaccount: None,
        },
        mint: mint(),
        ledger_id: Principal::from_slice(&[2, 1]),
        address: [3; 32].into(),
        amount: AMOUNT,
        token_fee: TOKEN_FEE,
        fee: 1_000_000,
        rent_reserve: RENT,
    }
}

fn message() -> Message {
    let token = SupportedSplToken {
        mint: mint(),
        token_program: spl_token_interface::id().to_bytes().into(),
        decimals: 6,
        ledger_id: terms().ledger_id,
        index_id: None,
        symbol: "ckTest".to_string(),
        minimum_deposit_amount: 1,
        deposit_fee: 0,
        deposits_paused: false,
        withdrawal_fee_token: TOKEN_FEE,
    };
    build_spl_withdrawal_message(
        &minter(),
        &[5; 32].into(),
        Hash::new_from_array([7; 32]),
        &SplWithdrawalTransfer {
            token: &token,
            destination: &terms().address,
            amount: TRANSFERRED,
        },
    )
    .unwrap()
}

fn index_of(address: Address) -> usize {
    message()
        .account_keys
        .iter()
        .position(|key| *key == address)
        .unwrap()
}

fn treasury_index() -> usize {
    index_of(associated_token_address(&minter(), &mint()))
}

fn destination_index() -> usize {
    index_of(associated_token_address(&terms().address, &mint()))
}

/// The metadata of a transaction that spent `rent` on a new destination token account
/// and moved `transferred` tokens. The destination had `destination_tokens` before.
struct Outcome {
    fee: Lamport,
    rent: Lamport,
    failed: bool,
    transferred: u64,
    destination_tokens: Option<u64>,
}

impl Outcome {
    fn success(rent: Lamport) -> Self {
        Self {
            fee: FEE,
            rent,
            failed: false,
            transferred: TRANSFERRED,
            destination_tokens: (rent == 0).then_some(40),
        }
    }

    fn meta(&self) -> UiTransactionStatusMeta {
        let accounts = message().account_keys.len();
        let mut pre = vec![1_000_000_u64; accounts];
        pre[0] = 10_000_000;
        pre[destination_index()] = if self.rent == 0 && !self.failed {
            RENT
        } else {
            0
        };
        let mut post = pre.clone();
        post[0] -= self.fee + self.rent;
        post[destination_index()] += self.rent;
        let token_balance = |index: usize, amount: u64| {
            let owner = if index == treasury_index() {
                minter()
            } else {
                terms().address
            };
            serde_json::json!({
                "accountIndex": index, "mint": mint().to_string(),
                "owner": owner.to_string(),
                "programId": spl_token_interface::id().to_string(),
                "uiTokenAmount": {"amount": amount.to_string(), "decimals": 6, "uiAmount": null, "uiAmountString": "0"}
            })
        };
        let mut pre_tokens = vec![token_balance(treasury_index(), 5_000)];
        let mut post_tokens = vec![token_balance(treasury_index(), 5_000 - self.transferred)];
        if let Some(tokens) = self.destination_tokens {
            pre_tokens.push(token_balance(destination_index(), tokens));
        }
        if !self.failed {
            post_tokens.push(token_balance(
                destination_index(),
                self.destination_tokens.unwrap_or(0) + self.transferred,
            ));
        }
        let error = if self.failed {
            serde_json::json!({"InstructionError": [2, {"Custom": 17}]})
        } else {
            serde_json::Value::Null
        };
        let status = if self.failed {
            serde_json::json!({"Err": error.clone()})
        } else {
            serde_json::json!({"Ok": null})
        };
        serde_json::from_value(serde_json::json!({
            "err": error, "status": status, "fee": self.fee,
            "preBalances": pre, "postBalances": post,
            "preTokenBalances": pre_tokens, "postTokenBalances": post_tokens,
        }))
        .unwrap()
    }
}

fn settle(
    meta: &UiTransactionStatusMeta,
) -> Result<SplWithdrawalOutcome, SplWithdrawalSettlementError> {
    settle_spl_withdrawal(&message(), meta, &terms())
}

#[test]
fn should_report_the_rent_of_a_new_destination_account() {
    assert_eq!(
        settle(&Outcome::success(RENT).meta()),
        Ok(SplWithdrawalOutcome::Succeeded {
            lamports_spent: FEE + RENT,
            rent_spent: RENT,
        })
    );
}

#[test]
fn should_report_no_rent_for_an_existing_destination_account() {
    assert_eq!(
        settle(&Outcome::success(0).meta()),
        Ok(SplWithdrawalOutcome::Succeeded {
            lamports_spent: FEE,
            rent_spent: 0,
        })
    );
}

#[test]
fn should_accept_a_rent_above_the_reserve_as_a_valid_outcome() {
    let rent = RENT + 160_720;
    assert_eq!(
        settle(&Outcome::success(rent).meta()),
        Ok(SplWithdrawalOutcome::Succeeded {
            lamports_spent: FEE + rent,
            rent_spent: rent,
        })
    );
}

#[test]
fn should_report_a_failed_transaction_with_its_fee_only() {
    let failed = Outcome {
        fee: FEE,
        rent: 0,
        failed: true,
        transferred: 0,
        destination_tokens: None,
    };
    assert_eq!(
        settle(&failed.meta()),
        Ok(SplWithdrawalOutcome::Failed {
            lamports_spent: FEE
        })
    );
    // A failed transaction cannot have spent rent.
    let mut meta = failed.meta();
    meta.post_balances[0] -= 1;
    assert!(matches!(
        settle(&meta),
        Err(SplWithdrawalSettlementError::UnexpectedBalanceChange { address, .. }) if address == minter()
    ));
}

#[test]
fn should_reject_lamport_changes_that_the_message_does_not_explain() {
    use SplWithdrawalSettlementError::*;
    // The fee payer lost more than the fee and the rent.
    let mut meta = Outcome::success(RENT).meta();
    meta.post_balances[0] -= 1;
    assert!(
        matches!(settle(&meta), Err(UnexpectedBalanceChange { address, .. }) if address == minter())
    );

    // The fee payer lost lamports that no account received as rent.
    let mut meta = Outcome::success(0).meta();
    meta.post_balances[0] -= RENT;
    assert!(
        matches!(settle(&meta), Err(UnexpectedBalanceChange { address, .. }) if address == minter())
    );

    // Another account changed.
    let other = (1..message().account_keys.len())
        .find(|index| *index != destination_index())
        .unwrap();
    for change in [-1_i64, 1] {
        let mut meta = Outcome::success(RENT).meta();
        meta.post_balances[other] = meta.post_balances[other]
            .checked_add_signed(change)
            .unwrap();
        assert!(matches!(
            settle(&meta),
            Err(UnexpectedBalanceChange { address, .. }) if address == message().account_keys[other]
        ));
    }

    // The destination token account lost lamports.
    let mut meta = Outcome::success(0).meta();
    meta.post_balances[destination_index()] -= 1;
    assert!(matches!(settle(&meta), Err(UnexpectedBalanceChange { .. })));
}

#[test]
fn should_reject_an_unexpected_fee_and_incomplete_balances() {
    use SplWithdrawalSettlementError::*;
    let mut meta = Outcome::success(RENT).meta();
    meta.fee = FEE + 1;
    assert_eq!(
        settle(&meta),
        Err(UnexpectedFee {
            expected: FEE,
            actual: FEE + 1
        })
    );

    let mut meta = Outcome::success(RENT).meta();
    meta.post_balances.pop();
    assert_eq!(settle(&meta), Err(IncompleteBalances));
    let mut meta = Outcome::success(RENT).meta();
    meta.pre_balances.clear();
    assert_eq!(settle(&meta), Err(IncompleteBalances));
}

#[test]
fn should_reject_token_balances_that_do_not_show_the_net_transfer() {
    use SplWithdrawalSettlementError::*;
    let treasury = associated_token_address(&minter(), &mint());
    let destination = associated_token_address(&terms().address, &mint());

    // The full amount moved, not the amount less the token fee.
    let wrong_amount = Outcome {
        transferred: AMOUNT,
        ..Outcome::success(RENT)
    };
    assert!(matches!(
        settle(&wrong_amount.meta()),
        Err(UnexpectedTokenBalanceChange { address, .. }) if address == treasury
    ));

    // The destination received another amount than the treasury sent.
    let mut meta = Outcome::success(0).meta();
    let OptionSerializer::Some(post) = &mut meta.post_token_balances else {
        unreachable!()
    };
    post[1].ui_token_amount.amount = "1".to_string();
    assert!(matches!(
        settle(&meta),
        Err(UnexpectedTokenBalanceChange { address, .. }) if address == destination
    ));

    // The metadata has no token balance for the treasury, or none at all.
    let mut meta = Outcome::success(RENT).meta();
    let OptionSerializer::Some(pre) = &mut meta.pre_token_balances else {
        unreachable!()
    };
    pre.clear();
    assert_eq!(settle(&meta), Err(IncompleteTokenBalances(treasury)));
    let mut meta = Outcome::success(RENT).meta();
    meta.post_token_balances = OptionSerializer::None;
    assert_eq!(settle(&meta), Err(IncompleteTokenBalances(treasury)));
    let mut meta = Outcome::success(RENT).meta();
    let OptionSerializer::Some(post) = &mut meta.post_token_balances else {
        unreachable!()
    };
    post.pop();
    assert_eq!(settle(&meta), Err(IncompleteTokenBalances(destination)));
}

#[test]
fn should_reject_a_message_without_the_expected_token_accounts() {
    let other_destination = SplWithdrawalRequestTerms {
        address: [4; 32].into(),
        ..terms()
    };
    assert_eq!(
        settle_spl_withdrawal(
            &message(),
            &Outcome::success(RENT).meta(),
            &other_destination
        ),
        Err(SplWithdrawalSettlementError::MissingTokenAccount(
            associated_token_address(&other_destination.address, &mint())
        ))
    );
}

#[test]
fn should_reject_token_balances_of_another_mint_owner_or_program() {
    use SplWithdrawalSettlementError::*;
    let treasury = associated_token_address(&minter(), &mint());
    let destination = associated_token_address(&terms().address, &mint());
    let other = Address::from([8; 32]).to_string();

    let mut meta = Outcome::success(0).meta();
    let OptionSerializer::Some(pre) = &mut meta.pre_token_balances else {
        unreachable!()
    };
    pre[0].mint = other.clone();
    assert_eq!(settle(&meta), Err(UnexpectedTokenAccount(treasury)));

    let mut meta = Outcome::success(0).meta();
    let OptionSerializer::Some(post) = &mut meta.post_token_balances else {
        unreachable!()
    };
    post[1].owner = OptionSerializer::Some(other.clone());
    assert_eq!(settle(&meta), Err(UnexpectedTokenAccount(destination)));

    let mut meta = Outcome::success(0).meta();
    let OptionSerializer::Some(post) = &mut meta.post_token_balances else {
        unreachable!()
    };
    post[0].program_id = OptionSerializer::Some(other);
    assert_eq!(settle(&meta), Err(UnexpectedTokenAccount(treasury)));

    // A balance without its owner or program is not complete.
    let mut meta = Outcome::success(0).meta();
    let OptionSerializer::Some(post) = &mut meta.post_token_balances else {
        unreachable!()
    };
    post[1].owner = OptionSerializer::None;
    assert_eq!(settle(&meta), Err(IncompleteTokenBalances(destination)));
}
