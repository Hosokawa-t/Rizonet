//! Auto-updater.
//!
//! Rizonet ships a minimal, opt-in updater. It fetches a JSON manifest
//! from a well-known URL, compares the version with the running binary
//! using [`semver`], downloads the new artifact if available, verifies
//! its SHA-256 checksum and stages it next to the current executable.
//!
//! The actual swap/restart step is platform-dependent and intentionally
//! left to the host application (the staged file path is returned).
//!
//! Manifest format (JSON):
//! ```json
//! {
//!   "version": "0.2.0",
//!   "notes": "What's new...",
//!   "artifacts": {
//!     "windows-x86_64": { "url": "https://.../app.exe", "sha256": "..." },
//!     "linux-x86_64":   { "url": "https://.../app",     "sha256": "..." },
//!     "macos-aarch64":  { "url": "https://.../app.dmg", "sha256": "..." }
//!   }
//! }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::{Result, RizonetError};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub artifacts: HashMap<String, Artifact>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Artifact {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub manifest: UpdateManifest,
    /// Artifact selected for the current platform key.
    pub artifact: Artifact,
    /// Platform key used for lookup (e.g. "windows-x86_64").
    pub platform: String,
}

/// Return the canonical platform key for the current target.
pub fn platform_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Check the manifest. Returns `Ok(Some(..))` if an update is available
/// for the current platform, `Ok(None)` otherwise.
pub async fn check(manifest_url: &str, current: &str) -> Result<Option<UpdateInfo>> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("rizonet-updater/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let manifest: UpdateManifest = client
        .get(manifest_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let current = Version::parse(current)?;
    let remote = Version::parse(&manifest.version)?;
    if remote <= current {
        return Ok(None);
    }
    let platform = platform_key();
    let artifact = manifest
        .artifacts
        .get(&platform)
        .cloned()
        .ok_or_else(|| RizonetError::Updater(format!("no artifact for platform {platform}")))?;
    Ok(Some(UpdateInfo {
        manifest,
        artifact,
        platform,
    }))
}

/// Download the update artifact, verify its checksum and stage it next
/// to the current executable. Returns the staged file path.
pub async fn download_and_stage(info: &UpdateInfo) -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|e| RizonetError::Updater(format!("cannot locate current exe: {e}")))?;
    let parent = exe
        .parent()
        .ok_or_else(|| RizonetError::Updater("current exe has no parent dir".into()))?;
    let staged = parent.join(format!(
        "{}.rizonet-update",
        exe.file_name().unwrap_or_default().to_string_lossy()
    ));

    download_with_checksum(&info.artifact.url, &info.artifact.sha256, &staged).await?;
    Ok(staged)
}

async fn download_with_checksum(url: &str, expected_sha256: &str, dest: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("rizonet-updater/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut resp = client.get(url).send().await?.error_for_status()?;

    // Write to a temp file, hash as we go.
    let tmp = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = resp.chunk().await? {
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);

    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(RizonetError::Checksum {
            expected: expected_sha256.to_string(),
            actual,
        });
    }

    // Atomically move into place.
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}
