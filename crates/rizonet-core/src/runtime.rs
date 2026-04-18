//! Hybrid Runtime Bootstrap.
//!
//! This is Rizonet's core differentiator. It detects the OS-provided
//! WebView and falls back to a bundled runtime (downloaded and cached
//! on first launch) when the system one is missing, outdated, or when
//! `strict` mode is configured.
//!
//! - Normal path: Tauri-like footprint using the system WebView.
//! - Fallback / strict: Electron-like consistency via a pinned runtime.

use directories::ProjectDirs;
use semver::{Version, VersionReq};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

use crate::config::{RuntimeConfig, RuntimeMode};
use crate::error::{Result, RizonetError};

/// Resolved runtime descriptor returned to the application layer.
#[derive(Debug, Clone)]
pub struct ResolvedRuntime {
    pub kind: RuntimeKind,
    /// Extra browser arguments to forward to the webview.
    pub extra_browser_args: Vec<String>,
    /// Root of the bundled runtime on disk (only present when `kind == Bundled`).
    pub runtime_root: Option<PathBuf>,
    /// Absolute path to the bundled browser executable folder (Windows: WebView2
    /// fixed-version layout). Forwarded as `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER`.
    pub browser_executable_folder: Option<PathBuf>,
    /// Detected WebView version, if any.
    pub detected_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    /// OS-provided WebView (Windows: WebView2, macOS: WKWebView, Linux: WebKitGTK).
    System,
    /// Bootstrapped, pinned runtime downloaded by Rizonet.
    Bundled,
}

/// Resolve a runtime according to the configuration. Downloads and
/// extracts the bundled runtime if necessary.
pub async fn resolve(cfg: &RuntimeConfig, app_id: &str) -> Result<ResolvedRuntime> {
    match cfg.mode {
        RuntimeMode::System => Ok(system_runtime()),
        RuntimeMode::Strict => bootstrap_bundled(cfg, app_id).await,
        RuntimeMode::Auto => match check_system(cfg) {
            SystemCheck::Ok(version) => {
                tracing::info!(?version, "using system webview");
                Ok(ResolvedRuntime {
                    detected_version: version,
                    ..system_runtime()
                })
            }
            SystemCheck::Unavailable(reason) => {
                tracing::warn!(%reason, "system webview unavailable — bootstrapping bundled runtime");
                bootstrap_bundled(cfg, app_id).await
            }
        },
    }
}

fn system_runtime() -> ResolvedRuntime {
    ResolvedRuntime {
        kind: RuntimeKind::System,
        extra_browser_args: vec![],
        runtime_root: None,
        browser_executable_folder: None,
        detected_version: None,
    }
}

enum SystemCheck {
    Ok(Option<String>),
    Unavailable(String),
}

fn check_system(cfg: &RuntimeConfig) -> SystemCheck {
    let version = detect_system_webview_version();
    match &version {
        Some(v) => {
            if let Some(req_str) = cfg.min_webview_version.as_deref() {
                match VersionReq::parse(&format!(">={req_str}")) {
                    Ok(req) => match Version::parse(&normalize_version(v)) {
                        Ok(actual) => {
                            if req.matches(&actual) {
                                SystemCheck::Ok(Some(v.clone()))
                            } else {
                                SystemCheck::Unavailable(format!(
                                    "system webview {v} < required {req_str}"
                                ))
                            }
                        }
                        Err(e) => SystemCheck::Unavailable(format!(
                            "cannot parse system version {v}: {e}"
                        )),
                    },
                    Err(e) => SystemCheck::Unavailable(format!("invalid min_webview_version: {e}")),
                }
            } else {
                SystemCheck::Ok(Some(v.clone()))
            }
        }
        None => SystemCheck::Unavailable("no system webview detected".into()),
    }
}

/// Normalize a dotted version string like "120.0.2210.91" into a strict
/// 3-component semver ("120.0.2210"). Missing components are padded with 0.
fn normalize_version(v: &str) -> String {
    let mut parts: Vec<String> = v.split('.').take(3).map(|s| s.to_string()).collect();
    while parts.len() < 3 {
        parts.push("0".into());
    }
    parts.join(".")
}

fn detect_system_webview_version() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        windows::webview2_version()
    }
    #[cfg(target_os = "macos")]
    {
        // WKWebView ships with the OS; we report the OS version as a proxy.
        Some(std::env::var("MACOS_VERSION").unwrap_or_else(|_| "system".into()))
    }
    #[cfg(target_os = "linux")]
    {
        linux::webkitgtk_version()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use winreg::enums::*;
    use winreg::RegKey;

    // Microsoft documents the Edge WebView2 Runtime client GUID as
    // {F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}.
    const GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

    pub fn webview2_version() -> Option<String> {
        let candidates = [
            (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{GUID}")),
            (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{GUID}")),
            (HKEY_CURRENT_USER,  format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{GUID}")),
        ];
        for (hive, path) in candidates.iter() {
            let root = RegKey::predef(*hive);
            if let Ok(key) = root.open_subkey(path) {
                if let Ok(v) = key.get_value::<String, _>("pv") {
                    if !v.is_empty() && v != "0.0.0.0" {
                        return Some(v);
                    }
                }
            }
        }
        None
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;

    pub fn webkitgtk_version() -> Option<String> {
        for name in &["libwebkit2gtk-4.1.so", "libwebkit2gtk-4.0.so"] {
            if which_lib(name).is_some() {
                return Some(name.trim_start_matches("libwebkit2gtk-").trim_end_matches(".so").into());
            }
        }
        None
    }

    fn which_lib(name: &str) -> Option<std::path::PathBuf> {
        for dir in [
            "/usr/lib",
            "/usr/lib64",
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib/aarch64-linux-gnu",
        ] {
            let p = Path::new(dir).join(name);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }
}

/// Download (if not cached), verify and extract the bundled runtime archive.
async fn bootstrap_bundled(cfg: &RuntimeConfig, app_id: &str) -> Result<ResolvedRuntime> {
    let url = cfg.bootstrap_url.as_deref().ok_or_else(|| {
        RizonetError::Runtime(
            "runtime.mode=strict (or auto-fallback) requires runtime.bootstrap_url".into(),
        )
    })?;

    let cache_dir = runtime_cache_dir(app_id)?;
    tokio::fs::create_dir_all(&cache_dir).await?;

    let archive_path = cache_dir.join("runtime.zip");
    let extract_dir = cache_dir.join("extracted");
    let stamp = cache_dir.join(".ok");

    if !stamp.exists() {
        tracing::info!(%url, "downloading Rizonet runtime");
        download(url, &archive_path).await?;

        if let Some(expected) = cfg.bootstrap_sha256.as_deref() {
            let actual = sha256_file(&archive_path).await?;
            if !actual.eq_ignore_ascii_case(expected) {
                let _ = tokio::fs::remove_file(&archive_path).await;
                return Err(RizonetError::Checksum {
                    expected: expected.to_string(),
                    actual,
                });
            }
        }

        // Reset extraction directory and unpack.
        if extract_dir.exists() {
            let _ = tokio::fs::remove_dir_all(&extract_dir).await;
        }
        tokio::fs::create_dir_all(&extract_dir).await?;

        let archive = archive_path.clone();
        let target = extract_dir.clone();
        tokio::task::spawn_blocking(move || extract_zip(&archive, &target))
            .await
            .map_err(|e| RizonetError::Runtime(format!("extraction task join error: {e}")))??;

        tokio::fs::write(&stamp, b"ok").await?;
        tracing::info!(?extract_dir, "runtime bootstrap complete");
    } else {
        tracing::debug!(?cache_dir, "runtime already cached");
    }

    let browser_folder = find_browser_folder(&extract_dir);

    Ok(ResolvedRuntime {
        kind: RuntimeKind::Bundled,
        extra_browser_args: vec![],
        runtime_root: Some(extract_dir),
        browser_executable_folder: browser_folder,
        detected_version: None,
    })
}

/// Locate the folder that should be passed to WebView2 as
/// `browser_executable_folder`. We accept any of these layouts:
/// - `<root>/msedgewebview2.exe`
/// - `<root>/EBWebView/msedgewebview2.exe`
/// - `<root>/*/msedgewebview2.exe`
fn find_browser_folder(root: &Path) -> Option<PathBuf> {
    let exe_name = if cfg!(target_os = "windows") {
        "msedgewebview2.exe"
    } else {
        return None; // non-Windows: bundled runtime shape is platform-specific
    };

    if root.join(exe_name).is_file() {
        return Some(root.to_path_buf());
    }
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() && p.join(exe_name).is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn runtime_cache_dir(app_id: &str) -> Result<PathBuf> {
    let dirs = ProjectDirs::from("net", "rizonet", app_id)
        .ok_or_else(|| RizonetError::Runtime("cannot resolve platform cache directory".into()))?;
    Ok(dirs.cache_dir().join("runtime"))
}

async fn download(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("rizonet/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut resp = client.get(url).send().await?.error_for_status()?;
    let mut file = tokio::fs::File::create(dest).await?;
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(())
}

fn extract_zip(src: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(src)?;
    let mut archive = zip::ZipArchive::new(file)?;
    archive.extract(dest)?;
    Ok(())
}

pub(crate) async fn sha256_file(path: &Path) -> Result<String> {
    use tokio::io::AsyncReadExt;
    let mut f = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}
