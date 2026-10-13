use candid::Principal;
use cksol_test_utils::PinnedDownload;
use flate2::read::GzDecoder;
use std::{
    env::var,
    fs,
    path::{Path, PathBuf},
};
use tar::Archive;
use tempfile::TempDir;

const STAGING_MINTER_ID: &str = "ljyxk-riaaa-aaaar-qb5mq-cai";
const SNAPSHOT: PinnedDownload = PinnedDownload {
    url: "https://dfinity-download-public.s3.eu-central-1.amazonaws.com/testdata/cksol/2026_10_13_cksol_staging_minter_snapshot_00000000000000000000000002300f590101.tar.gz",
    sha256: "5c9f9cb67bf331898f956bc85cd6cd5cb075e730b35e4658cf73fdae15878d04",
};

pub fn canister_id() -> Principal {
    Principal::from_text(STAGING_MINTER_ID).expect("the staging minter id is a valid principal")
}

pub async fn download_and_extract() -> TempDir {
    let cache_dir = cache_dir();
    let archive = SNAPSHOT.cached_or_downloaded(&cache_dir).await;
    let snapshot_dir = tempfile::Builder::new()
        .prefix("staging_minter_snapshot_")
        .tempdir_in(&cache_dir)
        .expect("failed to create a temporary directory for the snapshot");
    extract_without_top_level_dir(&archive, snapshot_dir.path());
    snapshot_dir
}

fn cache_dir() -> PathBuf {
    PathBuf::from(var("CARGO_MANIFEST_DIR").unwrap()).join("test_resources")
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
