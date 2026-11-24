use super::{InvalidStateError, State};
use candid::Principal;
use cksol_types::SplTokenInfo;
use cksol_types_internal::{SplTokenRegistration, SplTokenUpdate};
use solana_address::{Address, address};
use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
};

#[cfg(test)]
mod tests;

const CLASSIC_TOKEN_PROGRAM: Address = address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const MAX_SYMBOL_BYTES: usize = 20;

/// Immutable token identity and mutable deposit settings, rebuilt from upgrade events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportedSplToken {
    pub mint: Address,
    pub token_program: Address,
    pub decimals: u8,
    pub ledger_id: Principal,
    pub index_id: Option<Principal>,
    pub symbol: String,
    pub minimum_deposit_amount: u64,
    pub deposit_fee: u64,
    pub deposits_paused: bool,
}

fn invalid(message: impl Into<String>) -> InvalidStateError {
    InvalidStateError::InvalidSplToken(message.into())
}

fn parse_address(value: &str) -> Result<Address, InvalidStateError> {
    Address::from_str(value)
        .map_err(|error| invalid(format!("Invalid SPL address {value}: {error}")))
}

fn validate_canister_id(id: Principal) -> Result<(), InvalidStateError> {
    if id == Principal::anonymous() || id == Principal::management_canister() {
        return Err(invalid(
            "SPL ledger and index IDs cannot be anonymous or management principals",
        ));
    }
    Ok(())
}

impl SupportedSplToken {
    fn from_registration(args: SplTokenRegistration) -> Result<Self, InvalidStateError> {
        Ok(Self {
            mint: parse_address(&args.mint)?,
            token_program: parse_address(&args.token_program)?,
            decimals: args.decimals,
            ledger_id: args.ledger_id,
            index_id: args.index_id,
            symbol: args.symbol,
            minimum_deposit_amount: args.minimum_deposit_amount,
            deposit_fee: args.deposit_fee,
            deposits_paused: args.deposits_paused,
        })
    }

    fn validate(&self) -> Result<(), InvalidStateError> {
        if self.token_program != CLASSIC_TOKEN_PROGRAM {
            return Err(invalid("Only the classic SPL Token program is supported"));
        }

        validate_canister_id(self.ledger_id)?;
        if let Some(index_id) = self.index_id {
            validate_canister_id(index_id)?;
        }

        if self.symbol.trim().is_empty()
            || !self.symbol.is_ascii()
            || self.symbol.len() > MAX_SYMBOL_BYTES
        {
            return Err(invalid(
                "SPL symbol must be non-empty ASCII and at most 20 bytes",
            ));
        }

        self.validate_amounts()
    }

    fn update_deposit_settings(&mut self, update: SplTokenUpdate) {
        if let Some(minimum) = update.minimum_deposit_amount {
            self.minimum_deposit_amount = minimum;
        }
        if let Some(fee) = update.deposit_fee {
            self.deposit_fee = fee;
        }
        if let Some(paused) = update.deposits_paused {
            self.deposits_paused = paused;
        }
    }

    fn validate_amounts(&self) -> Result<(), InvalidStateError> {
        if self.minimum_deposit_amount == 0 {
            return Err(invalid("SPL minimum net deposit must be greater than zero"));
        }
        self.minimum_deposit_amount
            .checked_add(self.deposit_fee)
            .ok_or_else(|| invalid("SPL minimum deposit plus fee exceeds u64"))?;
        Ok(())
    }

    fn info(&self) -> SplTokenInfo {
        SplTokenInfo {
            mint: self.mint.into(),
            token_program: self.token_program.into(),
            decimals: self.decimals,
            ledger_id: self.ledger_id,
            index_id: self.index_id,
            symbol: self.symbol.clone(),
            minimum_deposit_amount: self.minimum_deposit_amount,
            deposit_fee: self.deposit_fee,
            deposits_paused: self.deposits_paused,
        }
    }
}

impl State {
    /// Supported SPL tokens in deterministic mint-address order.
    pub fn supported_spl_tokens(&self) -> Vec<SplTokenInfo> {
        self.supported_spl_tokens
            .values()
            .map(SupportedSplToken::info)
            .collect()
    }

    /// The registered identity and current deposit settings for one mint.
    pub fn supported_spl_token(&self, mint: &Address) -> Option<&SupportedSplToken> {
        self.supported_spl_tokens.get(mint)
    }

    /// Stages an entire registry update before changing the live registry.
    /// No RPC or ledger call is needed, including during replay.
    pub(super) fn updated_spl_tokens(
        &self,
        additions: Option<Vec<SplTokenRegistration>>,
        updates: Option<Vec<SplTokenUpdate>>,
    ) -> Result<BTreeMap<Address, SupportedSplToken>, InvalidStateError> {
        // Only this copy is mutated. State::upgrade assigns it before validating the complete state.
        let mut tokens = self.supported_spl_tokens.clone();
        let added_mints = register_tokens(&mut tokens, additions.unwrap_or_default())?;
        update_deposit_settings(&mut tokens, updates.unwrap_or_default(), &added_mints)?;

        Ok(tokens)
    }
}

/// Adds registrations without replacing an existing mint.
fn register_tokens(
    tokens: &mut BTreeMap<Address, SupportedSplToken>,
    additions: Vec<SplTokenRegistration>,
) -> Result<BTreeSet<Address>, InvalidStateError> {
    let mut added_mints = BTreeSet::new();
    for args in additions {
        let token = SupportedSplToken::from_registration(args)?;
        if tokens.contains_key(&token.mint) {
            return Err(invalid(format!(
                "SPL mint {} is already registered",
                token.mint
            )));
        }
        added_mints.insert(token.mint);
        tokens.insert(token.mint, token);
    }
    Ok(added_mints)
}

/// Updates each existing mint at most once. New registrations cannot also be updated.
fn update_deposit_settings(
    tokens: &mut BTreeMap<Address, SupportedSplToken>,
    updates: Vec<SplTokenUpdate>,
    added_mints: &BTreeSet<Address>,
) -> Result<(), InvalidStateError> {
    let mut updated_mints = BTreeSet::new();
    for args in updates {
        let mint = parse_address(&args.mint)?;
        if updated_mints.contains(&mint) {
            return Err(invalid(format!("Duplicate SPL update for {mint}")));
        }
        updated_mints.insert(mint);
        if added_mints.contains(&mint) {
            return Err(invalid(format!(
                "Cannot register and update SPL mint {mint} together"
            )));
        }
        let token = tokens
            .get_mut(&mint)
            .ok_or_else(|| invalid(format!("SPL mint {mint} is not registered")))?;
        token.update_deposit_settings(args);
    }
    Ok(())
}

/// Validates stored token settings and registry-wide canister identities.
/// Called by State::validate after applying upgrade settings.
pub(super) fn validate_tokens(
    tokens: &BTreeMap<Address, SupportedSplToken>,
    cksol_ledger_id: Principal,
    sol_rpc_canister_id: Principal,
) -> Result<(), InvalidStateError> {
    let mut canister_ids = BTreeSet::from([cksol_ledger_id, sol_rpc_canister_id]);
    for token in tokens.values() {
        token.validate()?;
        for id in std::iter::once(token.ledger_id).chain(token.index_id) {
            if !canister_ids.insert(id) {
                return Err(invalid(format!(
                    "SPL ledger or index {id} is already in use"
                )));
            }
        }
    }
    Ok(())
}
