use candid::{Decode, Encode, Principal};
use cksol_minter::state::{State, audit::replay_events, event::Event};
use cksol_types_internal::{
    SolanaNetwork,
    event::{GetEventsArgs, GetEventsResult},
};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use ic_agent::{Agent, identity::AnonymousIdentity};
use sha2::{Digest, Sha256};
use solana_address::{Address, address};
use std::{
    fs::{self, create_dir_all},
    io::{Read, Write},
    path::PathBuf,
};

/// The events are downloaded from [`MinterDeployment::Mainnet`]'s URL unless already cached.
/// To refresh them, run
/// `RETRIEVE_MINTER_EVENTS=true cargo test --locked --package cksol_minter --test replay_events should_replay_events_for_mainnet`,
/// upload the written file under a new version suffix and update the URL and hash.
#[tokio::test]
async fn should_replay_events_for_mainnet() {
    let state = MinterDeployment::Mainnet.replay_events().await;

    assert_eq!(state.solana_network(), SolanaNetwork::Mainnet);
    assert_eq!(
        minter_address(&state),
        address!("GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax")
    );
    assert_eq!(state.balance(), 995_500);
    assert_eq!(state.deposits().minted().len(), 1);
    assert_eq!(state.successful_withdrawal_requests().len(), 1);
    assert_eq!(state.succeeded_transactions().len(), 2);
    assert_eq!(state.nonce_pool_addresses().len(), 5);
}

/// The events are downloaded from [`MinterDeployment::Devnet`]'s URL unless already cached.
/// To refresh them, run
/// `RETRIEVE_MINTER_EVENTS=true cargo test --locked --package cksol_minter --test replay_events should_replay_events_for_devnet`,
/// upload the written file under a new version suffix and update the URL and hash.
#[tokio::test]
async fn should_replay_events_for_devnet() {
    let state = MinterDeployment::Devnet.replay_events().await;

    assert_eq!(state.solana_network(), SolanaNetwork::Devnet);
    assert_eq!(
        minter_address(&state),
        address!("Br8eRkeya8hy3sHCYqtWqGeNNZ349aPSUKGVYFWKKer1")
    );
    assert_eq!(state.balance(), 55_982_500);
    assert_eq!(state.deposits().minted().len(), 55);
    assert_eq!(state.deposits().dropped().len(), 18);
    assert_eq!(state.successful_withdrawal_requests().len(), 56);
    assert_eq!(state.succeeded_transactions().len(), 20);
    assert_eq!(state.nonce_pool_addresses().len(), 3);
}

fn minter_address(state: &State) -> Address {
    cksol_minter::address::minter_address(
        state
            .minter_public_key()
            .expect("the minter public key should have been fetched"),
    )
}

#[derive(Clone, Copy, Debug)]
enum MinterDeployment {
    Mainnet,
    Devnet,
}

impl MinterDeployment {
    const ICP_URL: &str = "https://icp0.io";
    const MAX_EVENTS_PER_QUERY: u64 = 2_000;

    fn minter_canister_id(self) -> Principal {
        let canister_id = match self {
            MinterDeployment::Mainnet => "lh22c-kyaaa-aaaar-qb5nq-cai",
            MinterDeployment::Devnet => "ljyxk-riaaa-aaaar-qb5mq-cai",
        };
        Principal::from_text(canister_id).unwrap()
    }

    fn events_url(self) -> &'static str {
        match self {
            MinterDeployment::Mainnet => {
                "https://download.dfinity.systems/testdata/cksol/mainnet_events_v1.gz"
            }
            MinterDeployment::Devnet => {
                "https://download.dfinity.systems/testdata/cksol/devnet_events_v1.gz"
            }
        }
    }

    fn events_sha256(self) -> &'static str {
        match self {
            MinterDeployment::Mainnet => {
                "4585d6a3ed633e7659936ae3e85cb23a9f966ce0240a586c8b0140f4af8385c0"
            }
            MinterDeployment::Devnet => {
                "a20ab9e3a312889f10745169fa7370a9bc9d052bf74de13dfe532924a5a51bcf"
            }
        }
    }

    fn events_file(self) -> PathBuf {
        let file_name = self.events_url().rsplit('/').next().unwrap();
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test_resources")
            .join(file_name)
    }

    async fn replay_events(self) -> State {
        let GetEventsResult {
            events,
            total_event_count,
        } = self.events().await;
        assert_eq!(events.len() as u64, total_event_count);
        let events = events
            .into_iter()
            .map(|event| Event::try_from(event).expect("Failed to convert event"));
        replay_events(events)
    }

    async fn events(self) -> GetEventsResult {
        let bytes = if retrieve_requested() {
            self.retrieve_and_store_events().await
        } else {
            self.cached_or_downloaded_events().await
        };
        decode_events(&bytes)
    }

    async fn cached_or_downloaded_events(self) -> Vec<u8> {
        let path = self.events_file();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => self.download_and_store_events().await,
        };
        let sha256 = hex::encode(Sha256::digest(&bytes));
        assert_eq!(
            sha256,
            self.events_sha256(),
            "SHA-256 of {} does not match the pinned hash of {}",
            path.display(),
            self.events_url()
        );
        bytes
    }

    async fn download_and_store_events(self) -> Vec<u8> {
        let url = self.events_url();
        let bytes = reqwest::get(url)
            .await
            .unwrap_or_else(|e| panic!("Failed to fetch {url}: {e:?}"))
            .bytes()
            .await
            .unwrap_or_else(|e| panic!("Failed to read bytes from {url}: {e:?}"))
            .to_vec();
        self.store(&bytes);
        bytes
    }

    async fn retrieve_and_store_events(self) -> Vec<u8> {
        let events = self.retrieve_all_events().await;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&Encode!(&events).unwrap()).unwrap();
        let bytes = encoder.finish().unwrap();
        let path = self.store(&bytes);
        println!(
            "Stored {} events of {self:?} minter to {}",
            events.total_event_count,
            path.display()
        );
        bytes
    }

    fn store(self, bytes: &[u8]) -> PathBuf {
        let path = self.events_file();
        create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes)
            .unwrap_or_else(|e| panic!("Failed to write {}: {e}", path.display()));
        path
    }

    async fn retrieve_all_events(self) -> GetEventsResult {
        let agent = Agent::builder()
            .with_url(Self::ICP_URL)
            .with_identity(AnonymousIdentity)
            .build()
            .expect("Failed to create agent");
        let mut events = Vec::new();
        let total_event_count = loop {
            let batch = self.get_events(&agent, events.len() as u64).await;
            events.extend(batch.events);
            if events.len() as u64 >= batch.total_event_count {
                break batch.total_event_count;
            }
        };
        GetEventsResult {
            events,
            total_event_count,
        }
    }

    async fn get_events(self, agent: &Agent, start: u64) -> GetEventsResult {
        let args = GetEventsArgs {
            start,
            length: Self::MAX_EVENTS_PER_QUERY,
        };
        let raw_result = agent
            .query(&self.minter_canister_id(), "get_events")
            .with_arg(Encode!(&args).unwrap())
            .call()
            .await
            .expect("Failed to call get_events");
        Decode!(&raw_result, GetEventsResult).expect("Failed to decode events")
    }
}

fn decode_events(bytes: &[u8]) -> GetEventsResult {
    let mut decoded = Vec::new();
    GzDecoder::new(bytes)
        .read_to_end(&mut decoded)
        .expect("Failed to decompress events");
    Decode!(&decoded, GetEventsResult).expect("Failed to decode events")
}

fn retrieve_requested() -> bool {
    std::env::var("RETRIEVE_MINTER_EVENTS")
        .ok()
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or_default()
}
