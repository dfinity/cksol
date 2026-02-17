use cksol_types::{Signature, UpdateBalanceArgs};
use std::str::FromStr;

const SOME_SIGNATURE: &str =
    "4basP1hZDqgt1BYwh29mURz4zr8BcJgya2Y4AjmzXB5vtViLG6hZRxF9iypkxkfCJXhJTFW7jU1PyG8rHXvYd4Zp";

pub fn default_update_balance_args() -> UpdateBalanceArgs {
    UpdateBalanceArgs {
        owner: None,
        subaccount: None,
        signature: some_signature(),
    }
}

pub fn some_signature() -> Signature {
    Signature::from_str(SOME_SIGNATURE).unwrap()
}
