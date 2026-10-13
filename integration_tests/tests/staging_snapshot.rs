use candid::Principal;
use cksol_int_tests::{CkSolMinter, Setup};
use cksol_test_utils::PinnedDownload;
use cksol_types_internal::{UpgradeArgs, event::EventType};
use flate2::read::GzDecoder;
use ic_management_canister_types::CanisterSettings;
use pocket_ic::{PocketIcBuilder, nonblocking::PocketIc};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tar::Archive;
use tempfile::TempDir;

const STAGING_MINTER_ID: &str = "ljyxk-riaaa-aaaar-qb5mq-cai";
const STAGING_MINTER_ADDRESS: &str = "Br8eRkeya8hy3sHCYqtWqGeNNZ349aPSUKGVYFWKKer1";
const SNAPSHOT: PinnedDownload = PinnedDownload {
    url: "https://dfinity-download-public.s3.eu-central-1.amazonaws.com/testdata/cksol/2026_10_13_cksol_staging_minter_snapshot_00000000000000000000000002300f590101.tar.gz",
    sha256: "5c9f9cb67bf331898f956bc85cd6cd5cb075e730b35e4658cf73fdae15878d04",
};

#[tokio::test]
async fn should_load_staging_snapshot_and_upgrade_to_current_wasm() {
    let snapshot_dir = download_and_extract_snapshot().await;
    let (env, canister_id) = load_snapshot_into_pocketic(snapshot_dir.path()).await;
    let minter = CkSolMinter::new(&env, canister_id);

    let state_before = minter.state().await;
    assert_eq!(
        state_before.info.minter_address.as_deref(),
        Some(STAGING_MINTER_ADDRESS)
    );
    assert!(
        !state_before.events.is_empty(),
        "the staging snapshot should contain at least one event"
    );

    let upgrade_args = UpgradeArgs::default();
    minter.stop().await;
    minter
        .upgrade(upgrade_args.clone())
        .await
        .expect("failed to upgrade the staging snapshot to the current wasm");
    minter.start().await;

    let state_after = minter.state().await;
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

async fn load_snapshot_into_pocketic(snapshot_dir: &Path) -> (PocketIc, Principal) {
    let env = PocketIcBuilder::new()
        .with_fiduciary_subnet()
        .build_async()
        .await;
    let controller = Setup::DEFAULT_CONTROLLER;
    let canister_id = Principal::from_text(STAGING_MINTER_ID)
        .expect("the staging minter id is a valid principal");
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
        .canister_snapshot_upload(canister_id, controller, None, snapshot_dir.to_path_buf())
        .await;
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

async fn download_and_extract_snapshot() -> TempDir {
    let cache_dir = snapshot_cache_dir();
    let archive = SNAPSHOT.cached_or_downloaded(&cache_dir).await;
    let snapshot_dir = tempfile::Builder::new()
        .prefix("staging_minter_snapshot_")
        .tempdir_in(&cache_dir)
        .expect("failed to create a temporary directory for the snapshot");
    extract_without_top_level_dir(&archive, snapshot_dir.path());
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
        let target_dir = target
            .parent()
            .expect("a snapshot archive entry always lives below the destination");
        fs::create_dir_all(target_dir)
            .unwrap_or_else(|e| panic!("failed to create {}: {e}", target_dir.display()));
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

fn snapshot_cache_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
}
