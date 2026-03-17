use crate::runtime::CanisterRuntime;
use cksol_types::InsufficientCyclesError;

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
