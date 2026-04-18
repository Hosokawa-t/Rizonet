//! Cross-thread application handle and Tao user events.

use std::sync::OnceLock;

use serde::Serialize;
use serde_json::Value;
use tao::event_loop::EventLoopProxy;

use crate::error::{Result, RizonetError};
use crate::state::StateMap;

/// Messages dispatched through the Tao event loop so that all webview/window
/// mutations happen on the UI thread regardless of where they were originated.
#[derive(Debug, Clone)]
pub enum UserEvent {
    /// Evaluate a JavaScript snippet in the main webview.
    EvalJs(String),
    /// Perform a window operation.
    Window(WindowOp),
    /// Open or close the devtools.
    Devtools(bool),
    /// Terminate the application.
    Exit,
}

#[derive(Debug, Clone)]
pub enum WindowOp {
    Show(bool),
    Minimize(bool),
    Maximize(bool),
    Focus,
    SetTitle(String),
    Close,
}

/// A cheap, cloneable, thread-safe handle to the running Rizonet app.
#[derive(Clone)]
pub struct AppHandle {
    proxy: EventLoopProxy<UserEvent>,
    pub(crate) state: StateMap,
}

impl AppHandle {
    pub(crate) fn new(proxy: EventLoopProxy<UserEvent>, state: StateMap) -> Self {
        Self { proxy, state }
    }

    /// Retrieve a previously-managed state value.
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<std::sync::Arc<T>> {
        self.state.get::<T>()
    }

    /// Emit an event to the frontend. JavaScript listeners registered via
    /// `window.__RIZONET__.on(event, cb)` will receive the payload.
    pub fn emit(&self, event: &str, payload: impl Serialize) -> Result<()> {
        let value: Value = serde_json::to_value(payload)?;
        let js = format!(
            "window.__RIZONET_EMIT__({}, {});",
            serde_json::to_string(event).unwrap_or_else(|_| "\"\"".into()),
            serde_json::to_string(&value).unwrap_or_else(|_| "null".into()),
        );
        self.eval_js(&js)
    }

    /// Evaluate an arbitrary JavaScript snippet in the main webview.
    pub fn eval_js(&self, script: &str) -> Result<()> {
        self.proxy
            .send_event(UserEvent::EvalJs(script.to_string()))
            .map_err(|_| RizonetError::Other("event loop closed".into()))
    }

    /// Toggle the window visibility.
    pub fn show(&self, visible: bool) -> Result<()> {
        self.send_window(WindowOp::Show(visible))
    }

    pub fn minimize(&self, minimized: bool) -> Result<()> {
        self.send_window(WindowOp::Minimize(minimized))
    }

    pub fn maximize(&self, maximized: bool) -> Result<()> {
        self.send_window(WindowOp::Maximize(maximized))
    }

    pub fn focus(&self) -> Result<()> {
        self.send_window(WindowOp::Focus)
    }

    pub fn set_title(&self, title: impl Into<String>) -> Result<()> {
        self.send_window(WindowOp::SetTitle(title.into()))
    }

    pub fn close(&self) -> Result<()> {
        self.send_window(WindowOp::Close)
    }

    /// Open (true) or close (false) the webview devtools.
    pub fn devtools(&self, open: bool) -> Result<()> {
        self.proxy
            .send_event(UserEvent::Devtools(open))
            .map_err(|_| RizonetError::Other("event loop closed".into()))
    }

    /// Gracefully exit the application.
    pub fn exit(&self) -> Result<()> {
        self.proxy
            .send_event(UserEvent::Exit)
            .map_err(|_| RizonetError::Other("event loop closed".into()))
    }

    fn send_window(&self, op: WindowOp) -> Result<()> {
        self.proxy
            .send_event(UserEvent::Window(op))
            .map_err(|_| RizonetError::Other("event loop closed".into()))
    }
}

/// Process-wide singleton holding the current [`AppHandle`]. Set once by the
/// event loop after the window and webview have been built.
pub(crate) static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Return the global [`AppHandle`]. Panics if called before the app has started.
pub fn handle() -> &'static AppHandle {
    APP_HANDLE
        .get()
        .expect("Rizonet AppHandle is not initialized yet")
}

/// Fallible accessor for code that may run before the event loop is ready.
pub fn try_handle() -> Option<&'static AppHandle> {
    APP_HANDLE.get()
}
