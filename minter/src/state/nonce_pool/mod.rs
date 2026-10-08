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
        match account.state {
            NonceAccountState::Free => {}
            NonceAccountState::Bound => {
                panic!("BUG: nonce account {address} is already bound to an in-flight transaction")
            }
        }
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
        match account.state {
            NonceAccountState::Bound => account.state = NonceAccountState::Free,
            NonceAccountState::Free => {
                panic!("BUG: cannot free nonce account {address} that is not bound")
            }
        }
    }

    /// Whether `nonce_value` was already bound to a transaction of this account,
    /// in which case a read returning it is stale.
    pub fn has_seen(&self, address: &Address, nonce_value: &Hash) -> bool {
        self.accounts
            .get(address)
            .is_some_and(|account| account.seen_nonce_values.contains(nonce_value))
    }

    /// Classifies a finalized read of `address` while the account is bound to an
    /// in-flight transaction carrying `bound_nonce_value`.
    pub fn classify_read(
        &self,
        address: &Address,
        bound_nonce_value: &Hash,
        read_nonce_value: &Hash,
    ) -> NonceRead {
        if read_nonce_value == bound_nonce_value {
            NonceRead::Unchanged
        } else if self.has_seen(address, read_nonce_value) {
            NonceRead::Stale
        } else {
            NonceRead::Advanced
        }
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

    pub fn free_accounts(&self) -> impl Iterator<Item = &Address> {
        self.accounts
            .iter()
            .filter(|(_, account)| account.is_free())
            .map(|(address, _)| address)
    }

    pub fn num_free_accounts(&self) -> usize {
        self.free_accounts().count()
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

impl NonceAccount {
    fn is_free(&self) -> bool {
        self.state == NonceAccountState::Free
    }
}

/// What a finalized read of a nonce account bound to an in-flight transaction proves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NonceRead {
    /// The account still stores the bound nonce value: the transaction has not landed.
    Unchanged,
    /// The account stores a value bound to an earlier transaction: the response lags behind.
    Stale,
    /// The account stores a value the minter never bound: the transaction has landed.
    Advanced,
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
