//! The JS ⇄ Rust IPC bridge.
//!
//! The frontend invokes Rust commands with `window.__RIZONET__.invoke(cmd, payload)`.
//! Each request is serialized as `{ id, cmd, payload }` and forwarded to Rust.
//! Rust dispatches the command and replies by evaluating
//! `window.__RIZONET_RESOLVE__(id, ok, value)` in the webview, which resolves
//! or rejects the JS promise returned by `invoke`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use crate::acl::CapabilitiesConfig;
use crate::error::Result;

/// A request received over the IPC channel.
#[derive(Debug, Clone, Deserialize)]
pub struct Invoke {
    pub id: u64,
    pub cmd: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct InvokeResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Each registered handler takes a payload and produces a JSON result.
pub type HandlerFn =
    Arc<dyn Fn(Value) -> Result<Value> + Send + Sync + 'static>;

#[derive(Default, Clone)]
pub struct IpcHandler {
    handlers: HashMap<String, HandlerFn>,
    pub(crate) capabilities: CapabilitiesConfig,
}

impl IpcHandler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<F>(mut self, cmd: impl Into<String>, f: F) -> Self
    where
        F: Fn(Value) -> Result<Value> + Send + Sync + 'static,
    {
        self.handlers.insert(cmd.into(), Arc::new(f));
        self
    }

    /// Override the capability policy. Normally set from `rizonet.config.json`.
    pub fn with_capabilities(mut self, cap: CapabilitiesConfig) -> Self {
        self.capabilities = cap;
        self
    }

    /// Register an already-boxed handler. Used by the framework to merge
    /// handlers between multiple [`IpcHandler`] instances.
    pub fn register_raw(mut self, cmd: String, f: HandlerFn) -> Self {
        self.handlers.insert(cmd, f);
        self
    }

    /// Iterate registered handlers by (cmd, handler).
    pub fn handlers_iter(&self) -> impl Iterator<Item = (String, HandlerFn)> + '_ {
        self.handlers.iter().map(|(k, v)| (k.clone(), v.clone()))
    }

    pub(crate) fn dispatch(&self, invoke: Invoke) -> InvokeResponse {
        if !self.capabilities.is_allowed(&invoke.cmd) {
            return InvokeResponse {
                id: invoke.id,
                ok: false,
                data: None,
                error: Some(format!("command denied by capabilities: {}", invoke.cmd)),
            };
        }
        match self.handlers.get(&invoke.cmd) {
            Some(h) => match h(invoke.payload) {
                Ok(data) => InvokeResponse {
                    id: invoke.id,
                    ok: true,
                    data: Some(data),
                    error: None,
                },
                Err(e) => InvokeResponse {
                    id: invoke.id,
                    ok: false,
                    data: None,
                    error: Some(e.to_string()),
                },
            },
            None => InvokeResponse {
                id: invoke.id,
                ok: false,
                data: None,
                error: Some(format!("unknown command: {}", invoke.cmd)),
            },
        }
    }
}

/// JavaScript shim injected into every webview. Sets up `window.__RIZONET__`
/// and the response dispatcher that Rust calls via `evaluate_script`.
pub const BRIDGE_SCRIPT: &str = r#"
(function () {
  if (window.__RIZONET__) return;
  const pending = new Map();
  const listeners = new Map(); // event name -> Set<fn>
  let nextId = 1;

  function on(event, cb) {
    let set = listeners.get(event);
    if (!set) {
      set = new Set();
      listeners.set(event, set);
    }
    set.add(cb);
    return () => off(event, cb);
  }
  function off(event, cb) {
    const set = listeners.get(event);
    if (set) set.delete(cb);
  }
  function once(event, cb) {
    const unsub = on(event, (p) => {
      unsub();
      cb(p);
    });
    return unsub;
  }

  window.__RIZONET__ = {
    version: "0.3.0",
    invoke(cmd, payload) {
      return new Promise((resolve, reject) => {
        const id = nextId++;
        pending.set(id, { resolve, reject });
        const msg = JSON.stringify({ id, cmd, payload: payload ?? null });
        window.ipc.postMessage(msg);
      });
    },
    on,
    off,
    once,
    emit(event, payload) {
      return this.invoke("event:emit", { event, payload: payload ?? null });
    },
  };

  window.__RIZONET_RESOLVE__ = function (id, ok, value) {
    const p = pending.get(id);
    if (!p) return;
    pending.delete(id);
    if (ok) p.resolve(value);
    else p.reject(new Error(typeof value === "string" ? value : JSON.stringify(value)));
  };

  window.__RIZONET_EMIT__ = function (event, payload) {
    const set = listeners.get(event);
    if (!set) return;
    for (const cb of set) {
      try { cb(payload); } catch (e) { console.error("rizonet listener error:", e); }
    }
  };
})();
"#;
