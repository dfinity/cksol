use crate::{Setup, SetupBuilder};
use assert_matches::assert_matches;
use cksol_types::{DepositStatus, ProcessDepositArgs, WithdrawalStatus};
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{InstallArgs, Lamport, OverrideProvider, RegexSubstitution, RoundingError};
use solana_address::Address;
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_config::{CommitmentConfig, RpcBlockConfig},
};
use solana_keypair::{Keypair, Signer};
use solana_signature::Signature;
use std::{
    net::{TcpListener, UdpSocket},
    ops::RangeInclusive,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicU16, Ordering},
    },
    time::Duration,
};

/// Solana base fee per signature included in a transaction.
pub const FEE_PER_SIGNATURE: Lamport = 5_000;

/// Minimum balance a zero-data Solana account must keep to stay rent-exempt.
/// A transfer that leaves an account with a smaller nonzero balance is rejected.
pub const RENT_EXEMPTION_THRESHOLD: Lamport = 890_880;

/// A `solana-test-validator` process owned by a single test.
///
/// Every validator listens on its own block of ports and writes to its own
/// ledger directory, so tests can run in parallel. The process is killed and
/// the ledger directory removed when the value is dropped.
pub struct SolanaTestValidator {
    process: Child,
    ledger_dir: PathBuf,
    rpc_url: String,
}

impl SolanaTestValidator {
    /// Starts a new validator and waits until it accepts transactions at the
    /// regular fee.
    ///
    /// Slots are shortened to 16 ticks so that a transaction is
    /// finalized within a few seconds, while a blockhash still stays valid for
    /// about 17 seconds, long enough for the minter to build and submit a
    /// transaction on it.
    ///
    /// # Panics
    ///
    /// Panics if `solana-test-validator` cannot be spawned or does not become
    /// ready within a minute.
    pub async fn start() -> Self {
        const TICKS_PER_SLOT: u16 = 16;
        let ports = ValidatorPorts::reserve();
        let ledger_dir =
            std::env::temp_dir().join(format!("cksol-solana-test-validator-{}", ports.rpc));
        let process = Command::new("solana-test-validator")
            .arg("--ledger")
            .arg(&ledger_dir)
            .arg("--reset")
            .arg("--quiet")
            .args(["--bind-address", "127.0.0.1"])
            .args(["--ticks-per-slot", &TICKS_PER_SLOT.to_string()])
            .args(["--rpc-port", &ports.rpc.to_string()])
            .args(["--faucet-port", &ports.faucet.to_string()])
            .args(["--gossip-port", &ports.gossip.to_string()])
            .args([
                "--dynamic-port-range",
                &format!("{}-{}", ports.dynamic.start(), ports.dynamic.end()),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to start solana-test-validator: is the Solana CLI installed?");
        let validator = Self {
            process,
            ledger_dir,
            rpc_url: format!("http://localhost:{}", ports.rpc),
        };
        validator.wait_until_ready().await;
        validator
    }

    /// The minter builds transactions on the block at the finalized slot rounded
    /// down by the SOL RPC canister. A freshly started validator charges no fee
    /// for its first blocks, so readiness means that block has the regular fee.
    async fn wait_until_ready(&self) {
        let rpc = self.rpc_client();
        let probe = Keypair::new();
        for _ in 0..240 {
            if self.fee_at_block_used_by_minter(&rpc, &probe).await == Some(FEE_PER_SIGNATURE) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!(
            "solana-test-validator at {} did not become ready",
            self.rpc_url
        );
    }

    async fn fee_at_block_used_by_minter(
        &self,
        rpc: &RpcClient,
        probe: &Keypair,
    ) -> Option<Lamport> {
        let finalized_slot = rpc
            .get_slot_with_commitment(CommitmentConfig::finalized())
            .await
            .ok()?;
        let block = rpc
            .get_block_with_config(
                RoundingError::default().round(finalized_slot),
                RpcBlockConfig {
                    rewards: Some(false),
                    ..RpcBlockConfig::default()
                },
            )
            .await
            .ok()?;
        let blockhash = block.blockhash.parse().ok()?;
        let transfer = solana_system_transaction::transfer(probe, &probe.pubkey(), 1, blockhash);
        rpc.get_fee_for_message(&transfer.message).await.ok()
    }

    /// Creates a test setup whose SOL RPC canister talks to this validator.
    pub async fn setup(&self) -> Setup {
        SetupBuilder::new()
            .with_proxy_canister()
            .with_pocket_ic_live_mode()
            .with_sol_rpc_install_args(InstallArgs {
                override_provider: Some(OverrideProvider {
                    override_url: Some(RegexSubstitution {
                        pattern: ".*".into(),
                        replacement: self.rpc_url().to_string(),
                    }),
                }),
                ..InstallArgs::default()
            })
            .build()
            .await
    }

    /// Deposits `amount` to the deposit address of `account`, has the minter
    /// process it, and returns the deposit address and the minted amount.
    pub async fn deposit_to_account(
        &self,
        setup: &Setup,
        account: Account,
        amount: Lamport,
    ) -> (Address, Lamport) {
        let expected_mint_amount = amount - Setup::DEFAULT_MANUAL_DEPOSIT_FEE;
        let deposit_address = setup.minter().get_deposit_address(account).await.into();

        println!("Depositing {amount} Lamport to address {deposit_address}");

        let balance_before = setup.ledger().balance_of(account).await;
        assert_eq!(balance_before, 0);

        let deposit_signature = self.transfer_to(deposit_address, amount).await;

        let result = setup
            .minter()
            .process_deposit(ProcessDepositArgs {
                owner: Some(account.owner),
                subaccount: account.subaccount,
                signature: deposit_signature.into(),
            })
            .await;
        assert_matches!(result, Ok(DepositStatus::Minted {
            minted_amount,
            deposit_id,
            block_index: _,
        }) if minted_amount == expected_mint_amount
            && deposit_id.signature == deposit_signature.into()
            && deposit_id.account == account);

        let balance_after = setup.ledger().balance_of(account).await;
        assert_eq!(balance_after, expected_mint_amount);

        (deposit_address, expected_mint_amount)
    }

    /// The JSON-RPC URL of this validator.
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// An RPC client for this validator at `confirmed` commitment.
    pub fn rpc_client(&self) -> RpcClient {
        RpcClient::new_with_commitment(self.rpc_url.clone(), CommitmentConfig::confirmed())
    }

    pub async fn get_balance(&self, address: &Address) -> Lamport {
        self.rpc_client()
            .get_balance(address)
            .await
            .expect("Failed to get Solana balance")
    }

    /// Whether the validator has ever processed the transaction, at any
    /// commitment level and with any outcome.
    pub async fn has_seen_transaction(&self, signature: &Signature) -> bool {
        self.rpc_client()
            .get_signature_status_with_commitment(signature, CommitmentConfig::processed())
            .await
            .expect("Failed to get signature status")
            .is_some()
    }

    pub async fn get_balances(&self, addresses: &[Address]) -> Vec<Lamport> {
        let mut balances = Vec::with_capacity(addresses.len());
        for address in addresses {
            balances.push(self.get_balance(address).await);
        }
        balances
    }

    /// Transfers `amount` to `address` from a freshly airdropped account and
    /// waits for the transfer to be finalized.
    pub async fn transfer_to(&self, address: Address, amount: Lamport) -> Signature {
        let sender = Keypair::new();
        let sender_funds = amount + FEE_PER_SIGNATURE + RENT_EXEMPTION_THRESHOLD;
        self.airdrop_and_confirm(sender.pubkey(), sender_funds)
            .await;

        let rpc = self.rpc_client();
        let recent_blockhash = rpc.get_latest_blockhash().await.unwrap();
        let transaction =
            solana_system_transaction::transfer(&sender, &address, amount, recent_blockhash);
        let signature = rpc.send_transaction(&transaction).await.unwrap();
        self.confirm_transaction(&signature, CommitmentConfig::finalized())
            .await;
        signature
    }

    pub async fn airdrop_and_confirm(&self, address: Address, airdrop_amount: Lamport) {
        let rpc = self.rpc_client();

        let balance_before = rpc.get_balance(&address).await.unwrap();

        let blockhash = rpc.get_latest_blockhash().await.unwrap();
        let airdrop_signature = rpc
            .request_airdrop_with_blockhash(&address, airdrop_amount, &blockhash)
            .await
            .unwrap();
        self.confirm_transaction(&airdrop_signature, CommitmentConfig::confirmed())
            .await;

        let balance_after = rpc.get_balance(&address).await.unwrap();
        assert_eq!(balance_after, balance_before + airdrop_amount);
    }

    /// Polls until the transaction reaches the given commitment.
    ///
    /// # Panics
    ///
    /// Panics if the transaction is not confirmed within 30 seconds.
    pub async fn confirm_transaction(&self, signature: &Signature, commitment: CommitmentConfig) {
        let rpc = self.rpc_client();
        for _ in 0..60 {
            let response = rpc
                .confirm_transaction_with_commitment(signature, commitment)
                .await;
            if let Ok(result) = response
                && result.value
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("Transaction {signature} not confirmed within timeout");
    }

    /// Polls a Solana address at `finalized` commitment until its balance exceeds
    /// `previous_balance`.
    ///
    /// # Panics
    ///
    /// Panics if the balance does not increase within a minute.
    pub async fn wait_for_finalized_balance(&self, address: &Address, previous_balance: Lamport) {
        for _ in 0..60 {
            let balance = self
                .rpc_client()
                .get_balance_with_commitment(address, CommitmentConfig::finalized())
                .await
                .map(|response| response.value)
                .unwrap_or(0);
            if balance > previous_balance {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        panic!(
            "Balance of {address} did not increase beyond {previous_balance} at finalized commitment"
        );
    }
}

/// Polls the minter until the given withdrawal is finalized, advancing time
/// between polls by enough for both the withdrawal and the finalization timer
/// to fire, without waiting for wall-clock minutes.
pub async fn wait_for_withdrawal_finalized(setup: &Setup, burn_index: u64) {
    for _ in 0..15 {
        if matches!(
            setup.minter().withdrawal_status(burn_index).await,
            WithdrawalStatus::TxFinalized(_)
        ) {
            return;
        }
        setup.advance_time_and_settle(Duration::from_mins(2)).await;
    }
    panic!("Withdrawal {burn_index} did not finalize within timeout");
}

impl Drop for SolanaTestValidator {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = std::fs::remove_dir_all(&self.ledger_dir);
    }
}

/// The ports a `solana-test-validator` binds: the JSON-RPC port together with
/// the WebSocket port right after it, the faucet, gossip, and a dynamic range
/// for the remaining services.
///
/// Blocks are handed out from a process-specific starting point so that test
/// binaries running at the same time start from different ports, and every
/// port of a block is probed before the block is used.
struct ValidatorPorts {
    rpc: u16,
    faucet: u16,
    gossip: u16,
    dynamic: RangeInclusive<u16>,
}

impl ValidatorPorts {
    const BLOCK_SIZE: u16 = 32;
    const FIRST_BLOCK: u16 = 20_000;
    const LAST_BLOCK: u16 = 60_000;
    const NUM_PROCESS_OFFSETS: u32 = 1_000;

    fn reserve() -> Self {
        static NEXT_BLOCK: OnceLock<AtomicU16> = OnceLock::new();
        let next_block = NEXT_BLOCK.get_or_init(|| AtomicU16::new(Self::first_block_of_process()));
        loop {
            let first = next_block.fetch_add(Self::BLOCK_SIZE, Ordering::SeqCst);
            assert!(
                first + Self::BLOCK_SIZE <= Self::LAST_BLOCK,
                "no free port block left for solana-test-validator"
            );
            if (first..first + Self::BLOCK_SIZE).all(is_free_port) {
                return Self {
                    rpc: first,
                    faucet: first + 2,
                    gossip: first + 3,
                    dynamic: first + 4..=first + Self::BLOCK_SIZE - 1,
                };
            }
        }
    }

    fn first_block_of_process() -> u16 {
        let offset = (std::process::id() % Self::NUM_PROCESS_OFFSETS) as u16;
        Self::FIRST_BLOCK + offset * Self::BLOCK_SIZE
    }
}

fn is_free_port(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok() && UdpSocket::bind(("127.0.0.1", port)).is_ok()
}
