use crate::runtime::CanisterRuntime;
use cksol_types::InsufficientCyclesError;

pub struct RpcCallCharge {
    pub attached_cycles: u128,
    pub fee_on_success: u128,
}

pub fn charge_rpc_call<R: CanisterRuntime, T, E>(
    runtime: &R,
    charge: RpcCallCharge,
    result: &Result<T, E>,
) {
    let rpc_cost = charge
        .attached_cycles
        .saturating_sub(runtime.msg_cycles_refunded());
    let fee = if result.is_ok() {
        charge.fee_on_success
    } else {
        0
    };
    charge_caller_cycles(runtime, rpc_cost + fee);
}

pub fn charge_caller_cycles<R: CanisterRuntime>(runtime: &R, amount: u128) {
    let cycles_received = runtime.msg_cycles_accept(amount);
    assert_eq!(
        cycles_received, amount,
        "Expected to receive {amount}, but got {cycles_received}"
    );
}

pub fn check_caller_available_cycles<R: CanisterRuntime>(
    runtime: &R,
    expected: u128,
) -> Result<u128, InsufficientCyclesError> {
    let available = runtime.msg_cycles_available();
    if available < expected {
        return Err(InsufficientCyclesError {
            expected,
            received: available,
        });
    }
    Ok(available)
}
