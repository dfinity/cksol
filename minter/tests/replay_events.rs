use candid::{Decode, Encode, Principal};
use cksol_types_internal::{
    SolanaNetwork,
    event::{Event, EventType, GetEventsArgs, GetEventsResult},
};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use ic_agent::{Agent, identity::AnonymousIdentity};
use std::{
    fs::{File, create_dir_all},
    io::{BufWriter, Read, Write},
    path::PathBuf,
    sync::LazyLock,
};

static MAINNET_EVENTS: LazyLock<GetEventsResult> =
    LazyLock::new(|| MinterDeployment::Mainnet.read_stored_events());
static DEVNET_EVENTS: LazyLock<GetEventsResult> =
    LazyLock::new(|| MinterDeployment::Devnet.read_stored_events());

/// To refresh the stored events of the production minter, run
/// `RETRIEVE_MINTER_EVENTS=true cargo test --locked --package cksol_minter --test replay_events should_replay_events_for_mainnet`.
#[tokio::test]
async fn should_replay_events_for_mainnet() {
    MinterDeployment::Mainnet
        .retrieve_and_store_events_if_env()
        .await;

    let events = &MAINNET_EVENTS;

    assert_eq!(events.events.len() as u64, events.total_event_count);
    assert_eq!(initial_network(events), SolanaNetwork::Mainnet);
}

/// To refresh the stored events of the staging minter, run
/// `RETRIEVE_MINTER_EVENTS=true cargo test --locked --package cksol_minter --test replay_events should_replay_events_for_devnet`.
#[tokio::test]
async fn should_replay_events_for_devnet() {
    MinterDeployment::Devnet
        .retrieve_and_store_events_if_env()
        .await;

    let events = &DEVNET_EVENTS;

    assert_eq!(events.events.len() as u64, events.total_event_count);
    assert_eq!(initial_network(events), SolanaNetwork::Devnet);
}

fn initial_network(events: &GetEventsResult) -> SolanaNetwork {
    match events.events.first() {
        Some(Event {
            payload: EventType::Init(init_args),
            ..
        }) => init_args.solana_network,
        other => panic!("the first event must be an Init event, got: {other:?}"),
    }
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

    fn events_file(self) -> PathBuf {
        let file_name = match self {
            MinterDeployment::Mainnet => "mainnet_events.gz",
            MinterDeployment::Devnet => "devnet_events.gz",
        };
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test_resources")
            .join(file_name)
    }

    async fn retrieve_and_store_events_if_env(self) {
        let retrieve_requested = std::env::var("RETRIEVE_MINTER_EVENTS")
            .ok()
            .and_then(|value| value.parse::<bool>().ok())
            .unwrap_or_default();
        if retrieve_requested {
            self.retrieve_and_store_events().await;
        }
    }

    async fn retrieve_and_store_events(self) {
        let events = self.retrieve_all_events().await;
        let path = self.events_file();
        create_dir_all(path.parent().unwrap()).unwrap();
        let mut encoder = GzEncoder::new(
            BufWriter::new(File::create(&path).unwrap()),
            Compression::best(),
        );
        encoder.write_all(&Encode!(&events).unwrap()).unwrap();
        encoder.finish().unwrap().flush().unwrap();
        println!(
            "Stored {} events of {self:?} minter to {}",
            events.total_event_count,
            path.display()
        );
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

    fn read_stored_events(self) -> GetEventsResult {
        let path = self.events_file();
        let file =
            File::open(&path).unwrap_or_else(|e| panic!("Failed to open {}: {e}", path.display()));
        let mut decoded = Vec::new();
        GzDecoder::new(file)
            .read_to_end(&mut decoded)
            .expect("Failed to decompress events");
        Decode!(&decoded, GetEventsResult).expect("Failed to decode events")
    }
}
