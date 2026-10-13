use sha2::{Digest, Sha256};
use std::{
    fmt::Display,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::NamedTempFile;

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// A test resource hosted on a public bucket, pinned by its URL and SHA-256.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinnedDownload {
    pub url: &'static str,
    pub sha256: &'static str,
}

impl PinnedDownload {
    /// The path under `cache_dir` holding the downloaded bytes, named after the last URL segment.
    pub fn cached_path(&self, cache_dir: &Path) -> PathBuf {
        let file_name = self.url.rsplit('/').next().unwrap_or(self.url);
        cache_dir.join(file_name)
    }

    /// Returns the bytes from the cache when present, otherwise downloads and caches them.
    ///
    /// # Panics
    ///
    /// Panics if the download fails or if the bytes, cached or downloaded,
    /// do not match the pinned SHA-256.
    pub async fn cached_or_downloaded(&self, cache_dir: &Path) -> Vec<u8> {
        let path = self.cached_path(cache_dir);
        match fs::read(&path) {
            Ok(bytes) => {
                self.assert_pinned_sha256(&bytes, path.display());
                bytes
            }
            Err(_) => self.download_and_store(&path).await,
        }
    }

    async fn download_and_store(&self, path: &Path) -> Vec<u8> {
        let url = self.url;
        let bytes = reqwest::Client::builder()
            .timeout(DOWNLOAD_TIMEOUT)
            .build()
            .expect("failed to create HTTP client")
            .get(url)
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .unwrap_or_else(|e| panic!("failed to fetch {url}: {e:?}"))
            .bytes()
            .await
            .unwrap_or_else(|e| panic!("failed to read bytes from {url}: {e:?}"))
            .to_vec();
        self.assert_pinned_sha256(&bytes, url);
        store_atomically(path, &bytes);
        bytes
    }

    fn assert_pinned_sha256(&self, bytes: &[u8], source: impl Display) {
        assert_eq!(
            sha256_hex(bytes),
            self.sha256,
            "SHA-256 of {source} does not match the pinned hash"
        );
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn store_atomically(path: &Path, bytes: &[u8]) {
    let dir = path
        .parent()
        .unwrap_or_else(|| panic!("{} has no parent directory", path.display()));
    fs::create_dir_all(dir).unwrap_or_else(|e| panic!("failed to create {}: {e}", dir.display()));
    let file = NamedTempFile::new_in(dir).unwrap_or_else(|e| {
        panic!(
            "failed to create a temporary file in {}: {e}",
            dir.display()
        )
    });
    fs::write(file.path(), bytes)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", file.path().display()));
    file.persist(path)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
}
