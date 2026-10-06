use solana_address::Address;
use solana_hash::Hash;
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

/// The pool of durable nonce accounts reserved for withdrawal transactions.
///
/// The accounts are created offline by the operators with the minter's main
/// address as the nonce authority and enter the pool through the init and
/// upgrade arguments; an account never leaves the pool. The pool may be empty,
/// in which case no withdrawal transaction can be submitted.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DurableNoncePool {
    accounts: BTreeMap<Address, NonceAccount>,
}

impl DurableNoncePool {
    pub fn new(addresses: impl IntoIterator<Item = Address>) -> Result<Self, NoncePoolError> {
        let mut pool = Self::default();
        pool.add_accounts(addresses)?;
        Ok(pool)
    }

    /// Adds the given accounts to the pool, or leaves the pool unchanged when
    /// any of them duplicates another one or an account already in the pool.
    pub fn add_accounts(
        &mut self,
        addresses: impl IntoIterator<Item = Address>,
    ) -> Result<(), NoncePoolError> {
        let mut additions = BTreeMap::new();
        for address in addresses {
            if self.accounts.contains_key(&address)
                || additions.insert(address, NonceAccount::default()).is_some()
            {
                return Err(NoncePoolError::DuplicateAccount(address));
            }
        }
        self.accounts.append(&mut additions);
        Ok(())
    }

    /// Binds the account to an in-flight withdrawal transaction carrying
    /// `nonce_value`, recording the value as seen.
    ///
    /// # Panics
    /// Panics if the account is already bound or if the nonce value was
    /// already bound to an earlier transaction of this account.
    pub(super) fn bind(&mut self, address: &Address, nonce_value: Hash) {
        let account = self.account_mut(address);
        assert_ne!(
            account.state,
            NonceAccountState::Bound,
            "BUG: nonce account {address} is already bound to an in-flight transaction"
        );
        assert!(
            account.seen_nonce_values.insert(nonce_value),
            "BUG: nonce value {nonce_value} of account {address} was already seen"
        );
        account.state = NonceAccountState::Bound;
    }

    /// Releases the account once its in-flight transaction has landed.
    ///
    /// # Panics
    /// Panics if the account is not bound.
    pub(super) fn free(&mut self, address: &Address) {
        let account = self.account_mut(address);
        assert_eq!(
            account.state,
            NonceAccountState::Bound,
            "BUG: cannot free nonce account {address} that is not bound"
        );
        account.state = NonceAccountState::Free;
    }

    pub fn addresses(&self) -> impl Iterator<Item = &Address> {
        self.accounts.keys()
    }

    pub fn contains(&self, address: &Address) -> bool {
        self.accounts.contains_key(address)
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    fn account_mut(&mut self, address: &Address) -> &mut NonceAccount {
        self.accounts
            .get_mut(address)
            .unwrap_or_else(|| panic!("BUG: nonce account {address} is not in the pool"))
    }
}

/// A durable nonce account of the pool with the nonce values the minter has
/// bound to transactions of this account, which classify every nonce read:
/// a never-seen value is the account's new frontier, while a seen value is
/// either the in-flight binding or a stale response.
#[derive(Debug, Default, PartialEq, Eq)]
struct NonceAccount {
    state: NonceAccountState,
    seen_nonce_values: BTreeSet<Hash>,
}

/// The lifecycle state of a durable nonce account in the pool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum NonceAccountState {
    /// The account is not bound to any in-flight withdrawal transaction.
    #[default]
    Free,
    /// The account is bound to an in-flight withdrawal transaction.
    Bound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoncePoolError {
    DuplicateAccount(Address),
}
