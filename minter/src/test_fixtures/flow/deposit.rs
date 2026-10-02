use crate::{
    state::read_state,
    test_fixtures::{
        events::{credit_sweep_at, queue, submit_sweep, succeed_transaction},
        queued_deposit_of,
    },
};
use cksol_types::DepositSolId;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::Lamport;
use solana_address::Address;
use solana_signature::Signature;

/// Entry point of the lifecycle of an automated deposit, applied event by event to the
/// state. Every stage holds what the test needs to assert on later.
pub struct DepositFlow;

impl DepositFlow {
    /// Queues a deposit of `account`, whose deposit address holds `sweepable_amount` on top
    /// of the rent exemption threshold, under the next deposit id of the state.
    pub fn queue(account: Account, sweepable_amount: Lamport) -> QueuedDepositFlow {
        let deposit_id = read_state(|state| state.deposits().next_id());
        let deposit = queued_deposit_of(account, sweepable_amount);
        queue(deposit_id, deposit);
        QueuedDepositFlow {
            deposit_id,
            account,
            address: deposit.address,
            sweepable_amount,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueuedDepositFlow {
    pub deposit_id: DepositSolId,
    pub account: Account,
    pub address: Address,
    pub sweepable_amount: Lamport,
}

impl QueuedDepositFlow {
    /// Submits a sweep of this single deposit under `signature`.
    pub fn sweep(self, signature: Signature) -> SubmittedSweepFlow {
        SweepFlow::of([self]).submit(signature)
    }
}

/// The deposits of one sweep transaction, in the order of its transfers.
pub struct SweepFlow {
    deposits: Vec<QueuedDepositFlow>,
}

impl SweepFlow {
    pub fn of(deposits: impl IntoIterator<Item = QueuedDepositFlow>) -> Self {
        Self {
            deposits: deposits.into_iter().collect(),
        }
    }

    /// Submits the sweep of the deposits to the minter's main address under `signature`.
    pub fn submit(self, signature: Signature) -> SubmittedSweepFlow {
        let deposit_ids = self.deposits.iter().map(|deposit| deposit.deposit_id);
        submit_sweep(signature, deposit_ids.collect());
        let expected_received = read_state(|state| {
            state
                .deposits()
                .swept()
                .get(&signature)
                .expect("BUG: the sweep was just submitted")
                .expected_received()
        });
        SubmittedSweepFlow {
            signature,
            deposits: self.deposits,
            expected_received,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedSweepFlow {
    pub signature: Signature,
    pub deposits: Vec<QueuedDepositFlow>,
    pub expected_received: Lamport,
}

impl SubmittedSweepFlow {
    /// Reports the sweep transaction as finalized without error.
    pub fn succeed(self) -> FinalizedSweepFlow {
        succeed_transaction(self.signature);
        FinalizedSweepFlow {
            signature: self.signature,
            deposits: self.deposits,
            expected_received: self.expected_received,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalizedSweepFlow {
    pub signature: Signature,
    pub deposits: Vec<QueuedDepositFlow>,
    pub expected_received: Lamport,
}

impl FinalizedSweepFlow {
    /// Credits the sweep with the amount its plan expected, at time zero.
    pub fn credit(self) -> CreditedSweepFlow {
        self.credit_at(0)
    }

    /// Credits the sweep with the amount its plan expected, at `timestamp`, which becomes
    /// the `created_at_time` of every pending mint.
    pub fn credit_at(self, timestamp: u64) -> CreditedSweepFlow {
        credit_sweep_at(self.signature, self.expected_received, timestamp);
        let mints = read_state(|state| {
            self.deposits
                .iter()
                .map(|deposit| {
                    let pending = state
                        .deposits()
                        .pending_mints()
                        .get(&deposit.deposit_id)
                        .expect("BUG: the credited deposit has no pending mint");
                    PendingMintFlow {
                        deposit_id: deposit.deposit_id,
                        account: pending.account(),
                        amount_to_mint: pending.amount_to_mint,
                        sweep_signature: pending.sweep_signature(),
                        created_at_time: pending.created_at_time,
                    }
                })
                .collect()
        });
        CreditedSweepFlow {
            signature: self.signature,
            mints,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreditedSweepFlow {
    pub signature: Signature,
    pub mints: Vec<PendingMintFlow>,
}

impl CreditedSweepFlow {
    /// The pending mint of a sweep of exactly one deposit.
    pub fn single_pending_mint(&self) -> PendingMintFlow {
        match self.mints.as_slice() {
            [mint] => *mint,
            mints => panic!(
                "BUG: expected the sweep {} to have exactly one pending mint, got {}",
                self.signature,
                mints.len()
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingMintFlow {
    pub deposit_id: DepositSolId,
    pub account: Account,
    pub amount_to_mint: Lamport,
    pub sweep_signature: Signature,
    pub created_at_time: u64,
}
