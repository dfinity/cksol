use solana_address::Address;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// The pool of durable nonce accounts reserved for withdrawal transactions.
///
/// The accounts are created offline by the operators with the minter's main
/// address as the nonce authority and enter or leave the pool through the init
/// and upgrade arguments. The pool may be empty, in which case no withdrawal
/// transaction can be submitted.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DurableNoncePool {
    accounts: BTreeMap<Address, NonceAccountState>,
}

impl DurableNoncePool {
    pub fn new(addresses: impl IntoIterator<Item = Address>) -> Result<Self, NoncePoolError> {
        let mut pool = Self::default();
        pool.add_accounts(addresses)?;
        Ok(pool)
    }

    pub fn add_accounts(
        &mut self,
        addresses: impl IntoIterator<Item = Address>,
    ) -> Result<(), NoncePoolError> {
        for address in addresses {
            if self
                .accounts
                .insert(address, NonceAccountState::Free)
                .is_some()
            {
                return Err(NoncePoolError::DuplicateAccount(address));
            }
        }
        Ok(())
    }

    /// Removes the given accounts from the pool.
    ///
    /// Only an account in the [`NonceAccountState::Free`] state may be removed,
    /// so an account bound to an in-flight transaction stays in the pool.
    pub fn remove_accounts(
        &mut self,
        addresses: impl IntoIterator<Item = Address>,
    ) -> Result<(), NoncePoolError> {
        for address in addresses {
            match self.accounts.get(&address) {
                None => return Err(NoncePoolError::UnknownAccount(address)),
                Some(NonceAccountState::Free) => {
                    self.accounts.remove(&address);
                }
            }
        }
        Ok(())
    }

    pub fn addresses(&self) -> impl Iterator<Item = &Address> {
        self.accounts.keys()
    }
}

/// The lifecycle state of a durable nonce account in the pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NonceAccountState {
    /// The account is not bound to any in-flight withdrawal transaction.
    Free,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoncePoolError {
    DuplicateAccount(Address),
    UnknownAccount(Address),
}
