//! Plugin API.
//!
//! Plugins are simple value types implementing [`Plugin`]. They can
//! register IPC commands, run startup hooks, and react to the
//! application lifecycle.
//!
//! ```ignore
//! use rizonet_core::{AppBuilder, Plugin, IpcHandler, RizonetError};
//! use serde_json::json;
//!
//! struct Clock;
//! impl Plugin for Clock {
//!     fn name(&self) -> &str { "clock" }
//!     fn register(&self, ipc: IpcHandler) -> IpcHandler {
//!         ipc.register("clock:now", |_| Ok(json!(chrono::Utc::now().to_rfc3339())))
//!     }
//! }
//! ```

use crate::error::Result;
use crate::ipc::IpcHandler;

/// A Rizonet plugin.
///
/// Plugins get a chance to mutate the [`IpcHandler`] (adding commands),
/// run at application startup and shutdown. Methods are all optional
/// except [`Plugin::name`].
pub trait Plugin: 'static {
    /// Unique plugin identifier. Used for logging and error messages.
    fn name(&self) -> &str;

    /// Called once, before the webview is built. Return the (possibly
    /// extended) `IpcHandler`.
    fn register(&self, ipc: IpcHandler) -> IpcHandler {
        ipc
    }

    /// Called once, after the webview has been built.
    fn on_start(&self) -> Result<()> {
        Ok(())
    }

    /// Called once, when the application is about to exit.
    fn on_exit(&self) {}
}
