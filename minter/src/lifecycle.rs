use cksol_types::lifecycle::{InitArgs, UpgradeArgs};

use crate::state::mutate_state;

pub fn init(args: InitArgs) {
    mutate_state(|s| {
        s.master_key_name = args.master_key_name;
    });
}

pub fn post_upgrade(args: Option<UpgradeArgs>) {
    if let Some(_args) = args {
        // apply upgrade args
    }
}