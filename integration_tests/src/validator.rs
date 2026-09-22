use sol_rpc_types::{Lamport, RoundingError};
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
    /// # Panics
    ///
    /// Panics if `solana-test-validator` cannot be spawned or does not become
    /// ready within a minute.
    pub async fn start() -> Self {
        let ports = ValidatorPorts::reserve();
        let ledger_dir =
            std::env::temp_dir().join(format!("cksol-solana-test-validator-{}", ports.rpc));
        let process = Command::new("solana-test-validator")
            .arg("--ledger")
            .arg(&ledger_dir)
            .arg("--reset")
            .arg("--quiet")
            .args(["--bind-address", "127.0.0.1"])
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
        self.airdrop_and_confirm(sender.pubkey(), 2 * amount).await;

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
