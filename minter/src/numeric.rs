use phantom_newtype::Id;

#[cfg(test)]
mod tests;

pub enum MintIndexTag {}
pub type LedgerMintIndex = Id<MintIndexTag, u64>;

pub enum BurnIndexTag {}
pub type LedgerBurnIndex = Id<BurnIndexTag, u64>;

/// The number of decimals of SOL, and of ckSOL which is backed 1:1 by SOL.
pub const SOL_DECIMALS: u8 = 9;

/// The number of lamports in one SOL.
pub const LAMPORTS_PER_SOL: u64 = 10_u64.pow(SOL_DECIMALS as u32);

/// Formats an amount in lamports as SOL, without trailing zeros, e.g. `1.5` for
/// 1,500,000,000 lamports. Used both on the dashboard and in the ICRC-21 consent
/// messages that users sign, so changing the format changes what users see before
/// signing a withdrawal.
pub fn lamports_to_sol(lamports: u64) -> String {
    let whole = lamports / LAMPORTS_PER_SOL;
    let frac = lamports % LAMPORTS_PER_SOL;
    if frac == 0 {
        format!("{whole}")
    } else {
        let frac_str = format!("{:0width$}", frac, width = SOL_DECIMALS as usize)
            .trim_end_matches('0')
            .to_string();
        format!("{whole}.{frac_str}")
    }
}
