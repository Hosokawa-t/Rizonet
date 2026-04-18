//! The Rizonet application-level API.
//!
//! This module provides:
//! - [`AppHandle`] — a cloneable, Send handle to the running app used to emit
//!   events, control the window, toggle devtools, etc. from any thread.
//! - [`UserEvent`] — messages sent through the Tao event loop to execute
//!   webview/window operations on the UI thread.
//! - [`builtins`] — a set of IPC commands (`dialog:*`, `notification:*`,
//!   `clipboard:*`, `shell:*`, `path:*`, `http:*`, `window:*`, `devtools:*`)
//!   that applications get for free.
//!
//! The public re-exports of this module live on the crate root.

pub mod builtins;
pub mod event;

pub use event::{AppHandle, UserEvent, WindowOp};
