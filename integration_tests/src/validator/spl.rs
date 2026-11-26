use crate::{
    Setup,
    fixtures::{DEFAULT_CALLER_ACCOUNT, RENT_EXEMPTION_THRESHOLD},
    validator::{FEE_PER_SIGNATURE, SolanaTestValidator},
};
use solana_address::Address;
use solana_client::rpc_config::CommitmentConfig;
use solana_keypair::{Keypair, Signer};
use solana_message::Instruction;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_program_pack::Pack;
use solana_system_interface::instruction::create_account;
use solana_transaction::Transaction;
use spl_associated_token_account_interface::{
    address::get_associated_token_address, instruction::create_associated_token_account,
};
use spl_token_interface::{
    instruction::{freeze_account, initialize_mint2, mint_to},
    state::Mint,
};
use std::time::Duration;

impl SolanaTestValidator {
    /// Funds the minter through its native deposit flow so the SOL is also tracked in state.
    /// The default caller receives the minted ckSOL.
    pub async fn fund_minter(&self, setup: &Setup) {
        self.fund_deposit_address(setup, DEFAULT_CALLER_ACCOUNT, LAMPORTS_PER_SOL)
            .await;
        let deposit_id = setup
            .minter()
            .deposit_sol(DEFAULT_CALLER_ACCOUNT)
            .await
            .expect("deposit_sol should queue the funding deposit");
        let balance = LAMPORTS_PER_SOL - RENT_EXEMPTION_THRESHOLD - FEE_PER_SIGNATURE;
        assert_eq!(setup.wait_for_deposit_minted(deposit_id).await, balance);
        assert_eq!(setup.minter().get_minter_info().await.balance, balance);
    }

    /// Creates a classic SPL mint on the validator, with `funder` as its mint and freeze
    /// authority.
    pub async fn create_mint(&self, funder: &Keypair, decimals: u8) -> Address {
        let mint = Keypair::new();
        let rent = self
            .rpc_client()
            .get_minimum_balance_for_rent_exemption(Mint::LEN)
            .await
            .unwrap();
        let instructions = [
            create_account(
                &funder.pubkey(),
                &mint.pubkey(),
                rent,
                Mint::LEN as u64,
                &spl_token_interface::id().to_bytes().into(),
            ),
            initialize_mint2(
                &spl_token_interface::id(),
                &mint.pubkey().to_bytes().into(),
                &funder.pubkey().to_bytes().into(),
                Some(&funder.pubkey().to_bytes().into()),
                decimals,
            )
            .unwrap(),
        ];
        self.send_and_finalize(&instructions, &[funder, &mint])
            .await;
        mint.pubkey()
    }

    /// Creates the ATA of `owner` for `mint` and returns its address.
    pub async fn create_token_account(
        &self,
        funder: &Keypair,
        mint: &Address,
        owner: &Address,
    ) -> Address {
        let instruction = create_associated_token_account(
            &funder.pubkey().to_bytes().into(),
            &owner.to_bytes().into(),
            &mint.to_bytes().into(),
            &spl_token_interface::id(),
        );
        self.send_and_finalize(&[instruction], &[funder]).await;
        get_associated_token_address(&owner.to_bytes().into(), &mint.to_bytes().into())
            .to_bytes()
            .into()
    }

    /// Creates the ATA of `owner` for `mint`, mints `amount` to it, and returns its address.
    pub async fn mint_to(
        &self,
        funder: &Keypair,
        mint: &Address,
        owner: &Address,
        amount: u64,
    ) -> Address {
        let account = self.create_token_account(funder, mint, owner).await;
        let instruction = mint_to(
            &spl_token_interface::id(),
            &mint.to_bytes().into(),
            &account.to_bytes().into(),
            &funder.pubkey().to_bytes().into(),
            &[],
            amount,
        )
        .unwrap();
        self.send_and_finalize(&[instruction], &[funder]).await;
        account
    }

    /// Freezes the token account, with `funder` as the freeze authority of `mint`.
    pub async fn freeze_token_account(&self, funder: &Keypair, mint: &Address, account: &Address) {
        let instruction = freeze_account(
            &spl_token_interface::id(),
            &account.to_bytes().into(),
            &mint.to_bytes().into(),
            &funder.pubkey().to_bytes().into(),
            &[],
        )
        .unwrap();
        self.send_and_finalize(&[instruction], &[funder]).await;
    }

    pub async fn get_token_balance(&self, address: &Address) -> u64 {
        self.rpc_client()
            .get_token_account_balance_with_commitment(address, CommitmentConfig::finalized())
            .await
            .expect("Failed to read the SPL token balance")
            .value
            .amount
            .parse()
            .unwrap()
    }

    /// Waits for the real token transfer to be finalized before the minter settles it.
    pub async fn wait_for_finalized_token_balance(&self, address: &Address, expected_balance: u64) {
        let rpc = self.rpc_client();
        let mut last_balance = None;
        for _ in 0..60 {
            last_balance = rpc
                .get_token_account_balance_with_commitment(address, CommitmentConfig::finalized())
                .await
                .ok()
                .and_then(|response| response.value.amount.parse::<u64>().ok());
            if last_balance == Some(expected_balance) {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        panic!(
            "Token balance of {address} at finalized commitment is {last_balance:?}, expected {expected_balance}"
        );
    }

    /// Sends the instructions with the first signer as the fee payer and waits for finality.
    async fn send_and_finalize(&self, instructions: &[Instruction], signers: &[&Keypair]) {
        let rpc = self.rpc_client();
        let blockhash = rpc.get_latest_blockhash().await.unwrap();
        let transaction = Transaction::new_signed_with_payer(
            instructions,
            Some(&signers[0].pubkey()),
            signers,
            blockhash,
        );
        let signature = rpc.send_transaction(&transaction).await.unwrap();
        self.confirm_transaction(&signature, CommitmentConfig::finalized())
            .await;
    }
}
