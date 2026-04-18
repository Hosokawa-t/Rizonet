//! Built-in IPC commands.
//!
//! Every Rizonet app automatically gets these commands unless the
//! `capabilities` config denies them:
//!
//! - **Dialog**:       `dialog:open_file`, `dialog:save_file`, `dialog:message`,
//!                     `dialog:confirm`
//! - **Notification**: `notification:send`
//! - **Clipboard**:    `clipboard:read_text`, `clipboard:write_text`
//! - **Shell**:        `shell:open`
//! - **Path**:         `path:home`, `path:config`, `path:data`, `path:cache`,
//!                     `path:documents`, `path:desktop`, `path:download`,
//!                     `path:temp`
//! - **HTTP**:         `http:fetch` (CORS-free HTTP client driven by Rust)
//! - **Window**:       `window:show`, `window:hide`, `window:minimize`,
//!                     `window:unminimize`, `window:maximize`,
//!                     `window:unmaximize`, `window:focus`,
//!                     `window:set_title`, `window:close`
//! - **Devtools**:     `devtools:open`, `devtools:close`
//! - **Event**:        `event:emit` (bounce an event back to the frontend)

use std::collections::HashMap;

use directories::{BaseDirs, UserDirs};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::api::event::handle;
use crate::error::RizonetError;
use crate::ipc::IpcHandler;

/// Register every built-in command on the provided handler.
pub fn register_all(mut ipc: IpcHandler) -> IpcHandler {
    ipc = register_dialog(ipc);
    ipc = register_notification(ipc);
    ipc = register_clipboard(ipc);
    ipc = register_shell(ipc);
    ipc = register_path(ipc);
    ipc = register_http(ipc);
    ipc = register_window(ipc);
    ipc = register_devtools(ipc);
    ipc = register_event(ipc);
    ipc
}

// ─────────────────────────────── Dialog ────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct OpenFileArgs {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    directory: bool,
    #[serde(default)]
    multiple: bool,
    #[serde(default)]
    filters: Vec<DialogFilter>,
    #[serde(default)]
    default_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SaveFileArgs {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    filters: Vec<DialogFilter>,
    #[serde(default)]
    default_path: Option<String>,
    #[serde(default)]
    file_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DialogFilter {
    name: String,
    extensions: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct MessageArgs {
    title: String,
    message: String,
    #[serde(default = "default_level")]
    level: String, // "info" | "warn" | "error"
}
fn default_level() -> String {
    "info".into()
}

fn build_file_dialog(
    title: &Option<String>,
    filters: &[DialogFilter],
    default_path: &Option<String>,
) -> rfd::FileDialog {
    let mut d = rfd::FileDialog::new();
    if let Some(t) = title.as_deref() {
        d = d.set_title(t);
    }
    if let Some(p) = default_path.as_deref() {
        d = d.set_directory(p);
    }
    for f in filters {
        let exts: Vec<&str> = f.extensions.iter().map(|s| s.as_str()).collect();
        d = d.add_filter(&f.name, &exts);
    }
    d
}

/// Run a dialog-producing closure on a dedicated OS thread.
///
/// `rfd`'s native dialogs must not be invoked directly from the webview's
/// UI thread because WebView2 (Windows) and WKWebView (macOS) keep their
/// own modal message pump that clashes with the OS file-picker, which
/// results in a crash or a hang. Running the dialog on a fresh thread
/// avoids the re-entry problem entirely; the IPC handler simply blocks
/// on the result, which is the normal UX for modal dialogs.
fn run_on_dialog_thread<F, T>(f: F) -> Result<T, RizonetError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::spawn(f)
        .join()
        .map_err(|_| RizonetError::Other("dialog thread panicked".into()))
}

fn register_dialog(ipc: IpcHandler) -> IpcHandler {
    ipc.register("dialog:open_file", |payload| {
        let args: OpenFileArgs = serde_json::from_value(payload).unwrap_or_default();
        run_on_dialog_thread(move || {
            let dialog = build_file_dialog(&args.title, &args.filters, &args.default_path);
            if args.directory {
                if args.multiple {
                    let paths = dialog.pick_folders().unwrap_or_default();
                    json!(paths
                        .into_iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>())
                } else {
                    match dialog.pick_folder() {
                        Some(p) => json!(p.display().to_string()),
                        None => Value::Null,
                    }
                }
            } else if args.multiple {
                let paths = dialog.pick_files().unwrap_or_default();
                json!(paths
                    .into_iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>())
            } else {
                match dialog.pick_file() {
                    Some(p) => json!(p.display().to_string()),
                    None => Value::Null,
                }
            }
        })
    })
    .register("dialog:save_file", |payload| {
        let args: SaveFileArgs = serde_json::from_value(payload).unwrap_or_default();
        run_on_dialog_thread(move || {
            let mut dialog = build_file_dialog(&args.title, &args.filters, &args.default_path);
            if let Some(name) = args.file_name.as_deref() {
                dialog = dialog.set_file_name(name);
            }
            match dialog.save_file() {
                Some(p) => json!(p.display().to_string()),
                None => Value::Null,
            }
        })
    })
    .register("dialog:message", |payload| {
        let args: MessageArgs = serde_json::from_value(payload)?;
        run_on_dialog_thread(move || {
            let level = match args.level.as_str() {
                "error" => rfd::MessageLevel::Error,
                "warn" | "warning" => rfd::MessageLevel::Warning,
                _ => rfd::MessageLevel::Info,
            };
            rfd::MessageDialog::new()
                .set_title(&args.title)
                .set_description(&args.message)
                .set_level(level)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            json!({ "ok": true })
        })
    })
    .register("dialog:confirm", |payload| {
        let args: MessageArgs = serde_json::from_value(payload)?;
        run_on_dialog_thread(move || {
            let result = rfd::MessageDialog::new()
                .set_title(&args.title)
                .set_description(&args.message)
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            json!(matches!(result, rfd::MessageDialogResult::Yes))
        })
    })
}

// ───────────────────────────── Notification ────────────────────────────

#[derive(Debug, Deserialize)]
struct NotifyArgs {
    title: String,
    body: String,
    #[serde(default)]
    subtitle: Option<String>,
}

fn register_notification(ipc: IpcHandler) -> IpcHandler {
    ipc.register("notification:send", |payload| {
        let args: NotifyArgs = serde_json::from_value(payload)?;
        let mut n = notify_rust::Notification::new();
        n.summary(&args.title).body(&args.body);
        if let Some(s) = args.subtitle {
            #[cfg(target_os = "macos")]
            n.subtitle(&s);
            #[cfg(not(target_os = "macos"))]
            let _ = s;
        }
        n.show()
            .map_err(|e| RizonetError::Other(format!("notification failed: {e}")))?;
        Ok(json!({ "ok": true }))
    })
}

// ───────────────────────────── Clipboard ───────────────────────────────

fn register_clipboard(ipc: IpcHandler) -> IpcHandler {
    ipc.register("clipboard:read_text", |_| {
        let mut cb = arboard::Clipboard::new()
            .map_err(|e| RizonetError::Other(format!("clipboard unavailable: {e}")))?;
        let text = cb.get_text().unwrap_or_default();
        Ok(json!(text))
    })
    .register("clipboard:write_text", |payload| {
        let s = payload
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RizonetError::Other("missing `text`".into()))?
            .to_string();
        let mut cb = arboard::Clipboard::new()
            .map_err(|e| RizonetError::Other(format!("clipboard unavailable: {e}")))?;
        cb.set_text(s)
            .map_err(|e| RizonetError::Other(format!("clipboard write failed: {e}")))?;
        Ok(json!({ "ok": true }))
    })
}

// ─────────────────────────────── Shell ─────────────────────────────────

fn register_shell(ipc: IpcHandler) -> IpcHandler {
    ipc.register("shell:open", |payload| {
        let target = payload
            .get("target")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RizonetError::Other("missing `target`".into()))?;
        opener::open(target)
            .map_err(|e| RizonetError::Other(format!("shell open failed: {e}")))?;
        Ok(json!({ "ok": true }))
    })
}

// ──────────────────────────────── Path ─────────────────────────────────

fn register_path(ipc: IpcHandler) -> IpcHandler {
    fn base_dir(name: &str) -> Value {
        let base = BaseDirs::new();
        let user = UserDirs::new();
        let path = match (name, base.as_ref(), user.as_ref()) {
            ("home", Some(b), _) => Some(b.home_dir().to_path_buf()),
            ("config", Some(b), _) => Some(b.config_dir().to_path_buf()),
            ("data", Some(b), _) => Some(b.data_dir().to_path_buf()),
            ("cache", Some(b), _) => Some(b.cache_dir().to_path_buf()),
            ("documents", _, Some(u)) => u.document_dir().map(|p| p.to_path_buf()),
            ("desktop", _, Some(u)) => u.desktop_dir().map(|p| p.to_path_buf()),
            ("download", _, Some(u)) => u.download_dir().map(|p| p.to_path_buf()),
            ("temp", _, _) => Some(std::env::temp_dir()),
            _ => None,
        };
        match path {
            Some(p) => json!(p.display().to_string()),
            None => Value::Null,
        }
    }

    for name in ["home", "config", "data", "cache", "documents", "desktop", "download", "temp"] {
        let ipc_name: String = format!("path:{name}");
        let key = name.to_string();
        // We cannot capture by move into a loop because `ipc` is consumed
        // each iteration; rebuild instead.
        let _ = (ipc_name, key);
    }
    // Unrolled for clarity and to dodge the capture issue above.
    ipc.register("path:home", |_| Ok(base_dir("home")))
        .register("path:config", |_| Ok(base_dir("config")))
        .register("path:data", |_| Ok(base_dir("data")))
        .register("path:cache", |_| Ok(base_dir("cache")))
        .register("path:documents", |_| Ok(base_dir("documents")))
        .register("path:desktop", |_| Ok(base_dir("desktop")))
        .register("path:download", |_| Ok(base_dir("download")))
        .register("path:temp", |_| Ok(base_dir("temp")))
}

// ──────────────────────────────── HTTP ─────────────────────────────────

#[derive(Debug, Deserialize)]
struct FetchArgs {
    #[serde(default = "default_method")]
    method: String,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}
fn default_method() -> String {
    "GET".into()
}

#[derive(Debug, Serialize)]
struct FetchResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
}

fn register_http(ipc: IpcHandler) -> IpcHandler {
    ipc.register("http:fetch", |payload| {
        let args: FetchArgs = serde_json::from_value(payload)?;
        // We spin up a tiny runtime per request so we do not require the
        // caller to be async. This keeps the public API simple.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(async move {
            let mut builder = reqwest::Client::builder();
            if let Some(ms) = args.timeout_ms {
                builder = builder.timeout(std::time::Duration::from_millis(ms));
            }
            let client = builder.build()?;
            let method = reqwest::Method::from_bytes(args.method.as_bytes())
                .map_err(|e| RizonetError::Other(format!("bad HTTP method: {e}")))?;
            let mut req = client.request(method, &args.url);
            for (k, v) in &args.headers {
                req = req.header(k, v);
            }
            if let Some(body) = args.body {
                req = req.body(body);
            }
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            let mut headers = HashMap::new();
            for (k, v) in resp.headers().iter() {
                if let Ok(s) = v.to_str() {
                    headers.insert(k.to_string(), s.to_string());
                }
            }
            let body = resp.text().await?;
            Ok(serde_json::to_value(FetchResponse { status, headers, body })?)
        })
    })
}

// ─────────────────────────────── Window ────────────────────────────────

fn register_window(ipc: IpcHandler) -> IpcHandler {
    ipc.register("window:show", |_| {
        handle().show(true)?;
        Ok(json!({ "ok": true }))
    })
    .register("window:hide", |_| {
        handle().show(false)?;
        Ok(json!({ "ok": true }))
    })
    .register("window:minimize", |_| {
        handle().minimize(true)?;
        Ok(json!({ "ok": true }))
    })
    .register("window:unminimize", |_| {
        handle().minimize(false)?;
        Ok(json!({ "ok": true }))
    })
    .register("window:maximize", |_| {
        handle().maximize(true)?;
        Ok(json!({ "ok": true }))
    })
    .register("window:unmaximize", |_| {
        handle().maximize(false)?;
        Ok(json!({ "ok": true }))
    })
    .register("window:focus", |_| {
        handle().focus()?;
        Ok(json!({ "ok": true }))
    })
    .register("window:close", |_| {
        handle().close()?;
        Ok(json!({ "ok": true }))
    })
    .register("window:set_title", |payload| {
        let title = payload
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RizonetError::Other("missing `title`".into()))?
            .to_string();
        handle().set_title(title)?;
        Ok(json!({ "ok": true }))
    })
}

// ───────────────────────────── Dev tools ───────────────────────────────

fn register_devtools(ipc: IpcHandler) -> IpcHandler {
    ipc.register("devtools:open", |_| {
        handle().devtools(true)?;
        Ok(json!({ "ok": true }))
    })
    .register("devtools:close", |_| {
        handle().devtools(false)?;
        Ok(json!({ "ok": true }))
    })
}

// ────────────────────────────── Events ─────────────────────────────────

#[derive(Debug, Deserialize)]
struct EmitArgs {
    event: String,
    #[serde(default)]
    payload: Value,
}

fn register_event(ipc: IpcHandler) -> IpcHandler {
    // Allow JS to re-emit events that Rust will broadcast back to JS. This
    // is useful when multiple webviews are added later.
    ipc.register("event:emit", |payload| {
        let args: EmitArgs = serde_json::from_value(payload)?;
        handle().emit(&args.event, args.payload)?;
        Ok(json!({ "ok": true }))
    })
}
