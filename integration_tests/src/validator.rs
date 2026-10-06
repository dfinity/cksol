use crate::{Setup, SetupBuilder, fixtures::RENT_EXEMPTION_THRESHOLD};
use cksol_types::WithdrawalStatus;
use icrc_ledger_types::icrc1::account::Account;
use sol_rpc_types::{InstallArgs, Lamport, OverrideProvider, RegexSubstitution, RoundingError};
use solana_address::Address;
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_config::{CommitmentConfig, RpcBlockConfig},
};
use solana_keypair::{Keypair, Signer};
use solana_signature::Signature;
use solana_system_interface::instruction::create_nonce_account;
use solana_transaction::Transaction;
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

/// Rent exemption minimum of a durable nonce account for its 80 bytes of state.
pub const NONCE_ACCOUNT_RENT_EXEMPTION: Lamport = 1_447_680;

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

    /// Transfers `amount` to the deposit address of `account`, waits for the
    /// transfer to be finalized, and returns the deposit address.
    pub async fn fund_deposit_address(
        &self,
        setup: &Setup,
        account: Account,
        amount: Lamport,
    ) -> Address {
        let deposit_address = setup.minter().get_deposit_address(account).await.into();

        println!("Depositing {amount} Lamport to address {deposit_address}");

        self.transfer_to(deposit_address, amount).await;
        self.wait_for_finalized_balance(&deposit_address, amount)
            .await;

        deposit_address
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

    /// The signatures of the transactions that mention `address`, newest first.
    pub async fn get_signatures_for_address(&self, address: &Address) -> Vec<Signature> {
        self.rpc_client()
            .get_signatures_for_address(address)
            .await
            .expect("Failed to get signatures for Solana address")
            .into_iter()
            .map(|transaction| {
                transaction
                    .signature
                    .parse()
                    .expect("BUG: the validator returned a malformed signature")
            })
            .collect()
    }

    pub async fn get_balances(&self, addresses: &[Address]) -> Vec<Lamport> {
        let mut balances = Vec::with_capacity(addresses.len());
        for address in addresses {
            balances.push(self.get_balance(address).await);
        }
        balances
    }

    /// Transfers `amount` to `address` from a freshly airdropped account and
    /// waits for the transfer to be finalized. The sender is funded so that it
    /// stays rent-exempt after the transfer, whatever the amount.
    pub async fn transfer_to(&self, address: Address, amount: Lamport) -> Signature {
        let sender = Keypair::new();
        self.airdrop_and_confirm(
            sender.pubkey(),
            amount + FEE_PER_SIGNATURE + RENT_EXEMPTION_THRESHOLD,
        )
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

    /// Creates `count` durable nonce accounts with `authority` as their nonce
    /// authority, funded by a freshly airdropped account, and returns their
    /// addresses once their creation is finalized.
    pub async fn create_nonce_accounts(&self, count: usize, authority: &Address) -> Vec<Address> {
        let funder = Keypair::new();
        let cost_per_account = NONCE_ACCOUNT_RENT_EXEMPTION + 2 * FEE_PER_SIGNATURE;
        self.airdrop_and_confirm(funder.pubkey(), 2 * count as u64 * cost_per_account)
            .await;

        let rpc = self.rpc_client();
        let mut addresses = Vec::with_capacity(count);
        for _ in 0..count {
            let nonce_account = Keypair::new();
            let instructions = create_nonce_account(
                &funder.pubkey(),
                &nonce_account.pubkey(),
                authority,
                NONCE_ACCOUNT_RENT_EXEMPTION,
            );
            let blockhash = rpc.get_latest_blockhash().await.unwrap();
            let transaction = Transaction::new_signed_with_payer(
                &instructions,
                Some(&funder.pubkey()),
                &[&funder, &nonce_account],
                blockhash,
            );
            let signature = rpc.send_transaction(&transaction).await.unwrap();
            self.confirm_transaction(&signature, CommitmentConfig::finalized())
                .await;
            addresses.push(nonce_account.pubkey());
        }
        addresses
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

    /// Polls a Solana address at `finalized` commitment until its balance is exactly
    /// `expected_balance`.
    ///
    /// # Panics
    ///
    /// Panics if the balance does not reach `expected_balance` within a minute, reporting
    /// the balance last seen.
    pub async fn wait_for_finalized_balance(&self, address: &Address, expected_balance: Lamport) {
        let mut last_balance = None;
        for _ in 0..60 {
            last_balance = self
                .rpc_client()
                .get_balance_with_commitment(address, CommitmentConfig::finalized())
                .await
                .map(|response| response.value)
                .ok();
            if last_balance == Some(expected_balance) {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        panic!(
            "Balance of {address} at finalized commitment is {last_balance:?}, expected {expected_balance}"
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
