use super::*;
use ic_canister_runtime::IcError;
use sol_rpc_types::JsonRpcError;

fn preflight_rejection(message: &str) -> SubmitTransactionError {
    json_rpc_error(PREFLIGHT_FAILURE_CODE, message)
}

fn json_rpc_error(code: i64, message: &str) -> SubmitTransactionError {
    SubmitTransactionError::RpcError(RpcError::JsonRpcError(JsonRpcError {
        code,
        message: message.to_string(),
    }))
}

#[test]
fn should_classify_on_chain_errors_of_the_transfer_instruction() {
    use FailedSplTransferReason::*;
    for (error, expected) in [
        (
            TransactionError::InstructionError(0, InstructionError::Custom(17)),
            FrozenAccount,
        ),
        (
            TransactionError::InstructionError(0, InstructionError::InvalidAccountData),
            MissingAccount,
        ),
        // Another token error, for example insufficient funds.
        (
            TransactionError::InstructionError(0, InstructionError::Custom(1)),
            Other,
        ),
        // The sweep has no second instruction.
        (
            TransactionError::InstructionError(1, InstructionError::Custom(17)),
            Other,
        ),
        (
            TransactionError::InstructionError(0, InstructionError::UninitializedAccount),
            Other,
        ),
        (TransactionError::AccountNotFound, Other),
        (TransactionError::InsufficientFundsForFee, Other),
    ] {
        assert_eq!(failed_sweep_reason(&error), expected, "{error:?}");
    }
}

#[test]
fn should_classify_preflight_rejections_by_the_texts_of_the_validator() {
    use FailedSplTransferReason::*;
    // The first two texts are copied from a local solana-test-validator.
    for (message, expected) in [
        (
            "Transaction simulation failed: Error processing Instruction 0: custom program error: 0x11",
            FrozenAccount,
        ),
        (
            "Transaction simulation failed: Error processing Instruction 0: invalid account data for instruction",
            MissingAccount,
        ),
        (
            "Transaction simulation failed: Error processing Instruction 0: custom program error: 0x1",
            Other,
        ),
        (
            "Transaction simulation failed: Error processing Instruction 0: custom program error: 0x110",
            Other,
        ),
        (
            "Transaction simulation failed: Error processing Instruction 1: custom program error: 0x11",
            Other,
        ),
        ("Transaction simulation failed: Blockhash not found", Other),
        ("custom program error: 0x11", Other),
        ("", Other),
    ] {
        assert_eq!(
            preflight_rejection_reason(&preflight_rejection(message)),
            Some(expected),
            "{message}"
        );
    }
}

#[test]
fn should_not_classify_submission_errors_that_may_still_land() {
    let frozen =
        "Transaction simulation failed: Error processing Instruction 0: custom program error: 0x11";
    for error in [
        // Node is unhealthy.
        json_rpc_error(-32005, frozen),
        json_rpc_error(-32603, "Internal error"),
        SubmitTransactionError::RpcError(RpcError::ValidationError(frozen.to_string())),
        SubmitTransactionError::IcError(IcError::CallPerformFailed),
        SubmitTransactionError::InconsistentRpcResults,
    ] {
        assert_eq!(preflight_rejection_reason(&error), None, "{error:?}");
    }
}
