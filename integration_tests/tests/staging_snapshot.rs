use candid::{Decode, Encode, Principal};
use cksol_int_tests::{Setup, cksol_minter_wasm};
use cksol_types::MinterInfo;
use cksol_types_internal::{
    MinterArg, UpgradeArgs,
    event::{Event, EventType, GetEventsArgs, GetEventsResult},
};
use flate2::read::GzDecoder;
use ic_management_canister_types::CanisterSettings;
use pocket_ic::{PocketIcBuilder, nonblocking::PocketIc};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tar::Archive;

const STAGING_MINTER_ID: &str = "ljyxk-riaaa-aaaar-qb5mq-cai";
const STAGING_MINTER_ADDRESS: &str = "Br8eRkeya8hy3sHCYqtWqGeNNZ349aPSUKGVYFWKKer1";
const SNAPSHOT_URL: &str = "https://dfinity-download-public.s3.eu-central-1.amazonaws.com/testdata/cksol/2026_10_13_cksol_staging_minter_snapshot_00000000000000000000000002300f590101.tar.gz";
const SNAPSHOT_SHA256: &str = "5c9f9cb67bf331898f956bc85cd6cd5cb075e730b35e4658cf73fdae15878d04";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_EVENTS_PER_QUERY: u64 = 2_000;

#[tokio::test]
async fn should_load_staging_snapshot_and_upgrade_to_current_wasm() {
    let snapshot_dir = download_and_extract_snapshot().await;
    let (env, canister_id) = load_snapshot_into_pocketic(snapshot_dir).await;

    let state_before = MinterState::query(&env, canister_id).await;
    assert_eq!(
        state_before.info.minter_address.as_deref(),
        Some(STAGING_MINTER_ADDRESS)
    );
    assert!(
        !state_before.events.is_empty(),
        "the staging snapshot should contain at least one event"
    );

    let upgrade_args = UpgradeArgs::default();
    env.stop_canister(canister_id, Some(Setup::DEFAULT_CONTROLLER))
        .await
        .expect("failed to stop canister before upgrade");
    env.upgrade_canister(
        canister_id,
        cksol_minter_wasm(),
        Encode!(&MinterArg::Upgrade(upgrade_args.clone())).unwrap(),
        Some(Setup::DEFAULT_CONTROLLER),
    )
    .await
    .expect("failed to upgrade the staging snapshot to the current wasm");
    env.start_canister(canister_id, Some(Setup::DEFAULT_CONTROLLER))
        .await
        .expect("failed to start canister after upgrade");

    let state_after = MinterState::query(&env, canister_id).await;
    assert_eq!(state_after.info, state_before.info);
    let (replayed_events, new_events) = state_after.events.split_at(state_before.events.len());
    assert_eq!(replayed_events, state_before.events);
    assert_eq!(
        new_events
            .iter()
            .map(|event| &event.payload)
            .collect::<Vec<_>>(),
        vec![&EventType::Upgrade(upgrade_args)]
    );

    env.drop().await;
}

#[derive(Debug)]
struct MinterState {
    info: MinterInfo,
    events: Vec<Event>,
}

impl MinterState {
    async fn query(env: &PocketIc, canister_id: Principal) -> Self {
        Self {
            info: get_minter_info(env, canister_id).await,
            events: get_all_events(env, canister_id).await,
        }
    }
}

async fn get_minter_info(env: &PocketIc, canister_id: Principal) -> MinterInfo {
    let bytes = env
        .query_call(
            canister_id,
            Principal::anonymous(),
            "get_minter_info",
            Encode!().unwrap(),
        )
        .await
        .expect("get_minter_info query failed");
    Decode!(&bytes, MinterInfo).unwrap()
}

async fn get_all_events(env: &PocketIc, canister_id: Principal) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let GetEventsResult {
            events: batch,
            total_event_count,
        } = get_events(env, canister_id, events.len() as u64).await;
        events.extend(batch);
        if events.len() as u64 >= total_event_count {
            break;
        }
    }
    events
}

async fn get_events(env: &PocketIc, canister_id: Principal, start: u64) -> GetEventsResult {
    let args = GetEventsArgs {
        start,
        length: MAX_EVENTS_PER_QUERY,
    };
    let bytes = env
        .query_call(
            canister_id,
            Principal::anonymous(),
            "get_events",
            Encode!(&args).unwrap(),
        )
        .await
        .expect("get_events query failed");
    Decode!(&bytes, GetEventsResult).unwrap()
}

async fn load_snapshot_into_pocketic(snapshot_dir: PathBuf) -> (PocketIc, Principal) {
    let env = PocketIcBuilder::new()
        .with_fiduciary_subnet()
        .build_async()
        .await;
    let controller = Setup::DEFAULT_CONTROLLER;
    let canister_id = Principal::from_text(STAGING_MINTER_ID).unwrap();
    env.create_canister_with_id(
        Some(controller),
        Some(CanisterSettings {
            controllers: Some(vec![controller]),
            ..CanisterSettings::default()
        }),
        canister_id,
    )
    .await
    .expect("failed to create canister at the staging minter id");
    env.add_cycles(canister_id, u128::MAX).await;

    let snapshot_id = env
        .canister_snapshot_upload(canister_id, controller, None, snapshot_dir.clone())
        .await;
    let _ = fs::remove_dir_all(&snapshot_dir);
    env.stop_canister(canister_id, Some(controller))
        .await
        .expect("failed to stop canister before loading the snapshot");
    env.load_canister_snapshot(canister_id, Some(controller), snapshot_id)
        .await
        .expect("failed to load the staging snapshot");
    env.start_canister(canister_id, Some(controller))
        .await
        .expect("failed to start canister after loading the snapshot");

    (env, canister_id)
}

async fn download_and_extract_snapshot() -> PathBuf {
    let archive = cached_or_downloaded_snapshot_archive().await;
    let snapshot_dir = snapshot_cache_dir().join(format!(
        "staging_minter_snapshot_{}_{}",
        std::process::id(),
        unique_suffix()
    ));
    let _ = fs::remove_dir_all(&snapshot_dir);
    extract_without_top_level_dir(&archive, &snapshot_dir);
    snapshot_dir
}

fn extract_without_top_level_dir(archive: &[u8], destination: &Path) {
    let mut entries = Archive::new(GzDecoder::new(archive));
    for entry in entries
        .entries()
        .expect("failed to read the snapshot archive")
    {
        let mut entry = entry.expect("failed to read a snapshot archive entry");
        let path = entry
            .path()
            .expect("snapshot archive entry has an invalid path")
            .into_owned();
        let relative_path: PathBuf = path.iter().skip(1).collect();
        if relative_path.as_os_str().is_empty() || is_apple_double_file(&relative_path) {
            continue;
        }
        let target = destination.join(relative_path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        entry
            .unpack(&target)
            .unwrap_or_else(|e| panic!("failed to extract {}: {e}", target.display()));
    }
}

fn is_apple_double_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("._"))
}

async fn cached_or_downloaded_snapshot_archive() -> Vec<u8> {
    let path = snapshot_cache_dir().join(SNAPSHOT_URL.rsplit('/').next().unwrap());
    match fs::read(&path) {
        Ok(bytes) if sha256_hex(&bytes) == SNAPSHOT_SHA256 => bytes,
        _ => download_and_store_snapshot_archive(&path).await,
    }
}

async fn download_and_store_snapshot_archive(path: &Path) -> Vec<u8> {
    let bytes = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .expect("failed to create HTTP client")
        .get(SNAPSHOT_URL)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .unwrap_or_else(|e| panic!("failed to fetch {SNAPSHOT_URL}: {e:?}"))
        .bytes()
        .await
        .unwrap_or_else(|e| panic!("failed to read bytes from {SNAPSHOT_URL}: {e:?}"))
        .to_vec();
    assert_eq!(
        sha256_hex(&bytes),
        SNAPSHOT_SHA256,
        "SHA-256 of {SNAPSHOT_URL} does not match the pinned hash"
    );
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, &bytes).unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
    bytes
}

fn snapshot_cache_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn unique_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
