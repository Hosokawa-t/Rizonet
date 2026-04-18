//! # Rizonet Core
//!
//! The ultimate desktop-app framework combining Tauri's footprint with
//! Electron's rendering consistency through a hybrid runtime bootstrap.
//!
//! ## Modules
//! - [`config`]  — parses `rizonet.config.json`.
//! - [`runtime`] — detects/bootstraps the WebView runtime.
//! - [`ipc`]     — the JS ⇄ Rust bridge.
//! - [`api`]     — [`AppHandle`], user events and the built-in commands.
//! - [`app`]     — the application builder and event loop.
//! - [`plugin`]  — the trait-based plugin API.
//! - [`updater`] — the minimal auto-updater.
//! - [`state`]   — type-indexed shared state.
//! - [`acl`]     — capability-based IPC access control.

pub mod acl;
pub mod api;
pub mod app;
pub mod config;
pub mod error;
pub mod ipc;
pub mod plugin;
pub mod runtime;
pub mod state;
pub mod updater;

#[cfg(feature = "iconify")]
pub mod iconify;

pub use acl::CapabilitiesConfig;
pub use api::event::{handle, try_handle, AppHandle, UserEvent, WindowOp};
pub use app::{App, AppBuilder};
pub use config::{RizonetConfig, RuntimeMode};
pub use error::{RizonetError, Result};
pub use ipc::{Invoke, IpcHandler};
pub use plugin::Plugin;
pub use runtime::{ResolvedRuntime, RuntimeKind};
pub use state::StateMap;
pub use updater::{platform_key, Artifact, UpdateInfo, UpdateManifest};
