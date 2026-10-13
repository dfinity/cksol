use candid::Principal;
use cksol_test_utils::PinnedDownload;
use flate2::read::GzDecoder;
use std::{
    env::var,
    path::{Path, PathBuf},
};
use tar::Archive;
use tempfile::TempDir;

const STAGING_MINTER_ID: &str = "ljyxk-riaaa-aaaar-qb5mq-cai";
const SNAPSHOT_ARCHIVE_ROOT: &str = "ljyxk";
const SNAPSHOT: PinnedDownload = PinnedDownload {
    url: "https://download.dfinity.systems/testdata/cksol/2026_10_13_cksol_staging_minter_snapshot_00000000000000000000000002300f590101.tar.gz",
    sha256: "5c9f9cb67bf331898f956bc85cd6cd5cb075e730b35e4658cf73fdae15878d04",
};

pub fn canister_id() -> Principal {
    Principal::from_text(STAGING_MINTER_ID).expect("the staging minter id is a valid principal")
}

pub async fn download_and_extract() -> TempDir {
    let cache_dir = cache_dir();
    let archive = SNAPSHOT.cached_or_downloaded(&cache_dir).await;
    let extraction_dir = tempfile::Builder::new()
        .prefix("staging_minter_snapshot_")
        .tempdir_in(&cache_dir)
        .expect("failed to create a temporary directory for the snapshot");
    Archive::new(GzDecoder::new(archive.as_slice()))
        .unpack(extraction_dir.path())
        .expect("failed to extract the snapshot archive");
    extraction_dir
}

pub fn dir(extraction_dir: &Path) -> PathBuf {
    extraction_dir.join(SNAPSHOT_ARCHIVE_ROOT)
}

fn cache_dir() -> PathBuf {
    PathBuf::from(var("CARGO_MANIFEST_DIR").unwrap()).join("test_resources")
}
