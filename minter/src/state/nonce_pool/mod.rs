use solana_address::Address;
use std::collections::BTreeMap;

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
}
