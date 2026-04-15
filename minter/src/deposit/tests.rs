use crate::test_fixtures::deposit::{
    DEPOSIT_ADDRESS, DEPOSIT_AMOUNT, deposit_transaction, deposit_transaction_to_wrong_address,
};
use assert_matches::assert_matches;
use solana_transaction_status_client_types::{EncodedTransaction, TransactionBinaryEncoding};

mod get_deposit_amount_tests {
    use super::*;
    use crate::deposit::{GetDepositAmountError, get_deposit_amount_to_address};

    #[test]
    fn should_fail_if_transaction_decoding_fails() {
        let mut transaction = deposit_transaction();
        transaction.transaction.transaction =
            EncodedTransaction::Binary("invalid".to_string(), TransactionBinaryEncoding::Base64);

        let result = get_deposit_amount_to_address(transaction, DEPOSIT_ADDRESS);

        assert_matches!(
            result,
            Err(GetDepositAmountError::TransactionParsingFailed(e)) => assert!(e.contains("Transaction decoding failed"))
        );
    }

    #[test]
    fn should_fail_if_transaction_has_no_meta() {
        let mut transaction = deposit_transaction();
        transaction.transaction.meta = None;

        let result = get_deposit_amount_to_address(transaction, DEPOSIT_ADDRESS);

        assert_eq!(result, Err(GetDepositAmountError::NoMetaField));
    }

    #[test]
    fn should_fail_if_transaction_deposit_to_wrong_address() {
        let transaction = deposit_transaction_to_wrong_address();

        let result = get_deposit_amount_to_address(transaction, DEPOSIT_ADDRESS);

        assert_eq!(
            result,
            Err(GetDepositAmountError::DepositAddressNotInAccountKeys)
        );
    }

    #[test]
    fn should_succeed_for_valid_deposit() {
        let transaction = deposit_transaction();

        let result = get_deposit_amount_to_address(transaction, DEPOSIT_ADDRESS);

        assert_eq!(result, Ok(DEPOSIT_AMOUNT));
    }
}
