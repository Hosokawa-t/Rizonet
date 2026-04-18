use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::acl::CapabilitiesConfig;
use crate::error::{Result, RizonetError};

/// Top-level schema for `rizonet.config.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RizonetConfig {
    pub app: AppConfig,
    #[serde(default)]
    pub window: WindowConfig,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub capabilities: CapabilitiesConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub name: String,
    pub identifier: String,
    /// URL loaded in development mode (e.g. "http://localhost:5173").
    #[serde(default)]
    pub dev_url: Option<String>,
    /// Directory containing the built web assets (e.g. "dist").
    #[serde(default = "default_dist")]
    pub dist_dir: String,
}

fn default_dist() -> String {
    "dist".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    #[serde(default = "default_title")]
    pub title: String,
    #[serde(default = "default_width")]
    pub width: f64,
    #[serde(default = "default_height")]
    pub height: f64,
    #[serde(default = "default_true")]
    pub resizable: bool,
    #[serde(default)]
    pub fullscreen: bool,
    /// Start the window maximized (ignored when `fullscreen` is true).
    #[serde(default)]
    pub maximized: bool,
    #[serde(default = "default_true")]
    pub decorations: bool,
    #[serde(default)]
    pub transparent: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: default_title(),
            width: default_width(),
            height: default_height(),
            resizable: true,
            fullscreen: false,
            maximized: false,
            decorations: true,
            transparent: false,
        }
    }
}

fn default_title() -> String {
    "Rizonet App".into()
}
fn default_width() -> f64 {
    1024.0
}
fn default_height() -> f64 {
    768.0
}
fn default_true() -> bool {
    true
}

/// Runtime strategy:
/// - `auto`   : Use the OS WebView when available, otherwise download the
///             bundled runtime (default).
/// - `system` : Always use the OS WebView (Tauri-like).
/// - `strict` : Always download and use the bundled runtime (Electron-like
///             rendering consistency).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_mode")]
    pub mode: RuntimeMode,
    /// Minimum required system WebView version. When the detected version is
    /// below this, `auto` mode falls back to the bundled runtime.
    #[serde(default)]
    pub min_webview_version: Option<String>,
    /// URL from which to download the bundled runtime (HTTPS recommended).
    #[serde(default)]
    pub bootstrap_url: Option<String>,
    /// SHA-256 checksum of the downloaded runtime archive.
    #[serde(default)]
    pub bootstrap_sha256: Option<String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::Auto,
            min_webview_version: None,
            bootstrap_url: None,
            bootstrap_sha256: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeMode {
    Auto,
    System,
    Strict,
}

fn default_mode() -> RuntimeMode {
    RuntimeMode::Auto
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildConfig {
    #[serde(default)]
    pub before_dev: Option<String>,
    #[serde(default)]
    pub before_build: Option<String>,
    #[serde(default)]
    pub targets: Vec<String>,
}

impl RizonetConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| RizonetError::Config(format!("cannot read {:?}: {e}", path)))?;
        let cfg: RizonetConfig = serde_json::from_str(&text)
            .map_err(|e| RizonetError::Config(format!("invalid JSON in {:?}: {e}", path)))?;
        Ok(cfg)
    }

    /// Walk up from `start` looking for a `rizonet.config.json` file.
    /// Returns the parsed config together with the resolved path.
    pub fn discover(start: impl AsRef<Path>) -> Result<(Self, PathBuf)> {
        let start = start.as_ref();
        let mut cur = Some(start.to_path_buf());
        while let Some(dir) = cur {
            let candidate = dir.join("rizonet.config.json");
            if candidate.is_file() {
                let cfg = Self::load(&candidate)?;
                return Ok((cfg, candidate));
            }
            cur = dir.parent().map(|p| p.to_path_buf());
        }
        Err(RizonetError::Config(
            "rizonet.config.json not found in any ancestor directory".into(),
        ))
    }
}
