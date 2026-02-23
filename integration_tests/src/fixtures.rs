use cksol_types::{Signature, UpdateBalanceArgs};
use std::str::FromStr;

pub const DEPOSIT_TRANSACTION_SIGNATURE: &str =
    "5nAMoTjRdRw4ah4WS7FPipqn3HYqZz9FMTLheVmN6mnJjgqFfComsZeAgBa6FBbX3bf5TNMegPjPE3PYQPCHup2s";

pub fn default_update_balance_args() -> UpdateBalanceArgs {
    UpdateBalanceArgs {
        owner: None,
        subaccount: None,
        signature: deposit_transaction_signature(),
    }
}

pub fn deposit_transaction_signature() -> Signature {
    Signature::from_str(DEPOSIT_TRANSACTION_SIGNATURE).unwrap()
}
