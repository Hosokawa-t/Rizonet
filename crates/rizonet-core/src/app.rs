//! Application bootstrap.
//!
//! Builds the Tao event loop + Wry WebView, wires up the Rizonet IPC bridge,
//! registers all built-in commands, applies the resolved runtime (including
//! fixed-version WebView2 on Windows when bootstrapped), and exposes a
//! cross-thread [`AppHandle`] to the rest of the app.

use std::borrow::Cow;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::http::{Request, Response};
use wry::{WebView, WebViewBuilder};

/// Custom URL scheme used to serve the application's static assets.
///
/// We deliberately avoid the `file://` scheme on Windows because wry builds
/// an `http::Request` for IPC messages whose URI is the source document's
/// URL. `http::Uri` treats the colon in `file:///C:/...` as a port
/// separator and rejects it, which panics inside an `extern "C"` callback
/// — i.e. immediately aborts the process. Routing through a custom scheme
/// produces `http://rizonet-asset.localhost/...` internally on Windows,
/// which `http::Uri` parses without issue.
const ASSET_SCHEME: &str = "rizonet-asset";
const ASSET_HOST: &str = "localhost";

use crate::api::builtins;
use crate::api::event::{AppHandle, UserEvent, WindowOp, APP_HANDLE};
use crate::config::RizonetConfig;
use crate::error::Result;
use crate::ipc::{self, Invoke, IpcHandler};
use crate::plugin::Plugin;
use crate::runtime::{self, ResolvedRuntime};
use crate::state::StateMap;

pub struct AppBuilder {
    config: RizonetConfig,
    ipc: IpcHandler,
    plugins: Vec<Box<dyn Plugin>>,
    state: StateMap,
    dev: bool,
    project_root: Option<PathBuf>,
}

impl AppBuilder {
    pub fn new(config: RizonetConfig) -> Self {
        Self {
            config,
            ipc: IpcHandler::new(),
            plugins: Vec::new(),
            state: StateMap::new(),
            dev: false,
            project_root: None,
        }
    }

    pub fn ipc(mut self, handler: IpcHandler) -> Self {
        self.ipc = handler;
        self
    }

    /// Merge additional commands on top of the current handler. Convenient
    /// when registering built-ins and user commands separately.
    pub fn with_ipc<F>(mut self, f: F) -> Self
    where
        F: FnOnce(IpcHandler) -> IpcHandler,
    {
        self.ipc = f(self.ipc);
        self
    }

    pub fn plugin<P: Plugin>(mut self, plugin: P) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    /// Insert a value into the app's shared state.
    pub fn manage<T: Send + Sync + 'static>(self, value: T) -> Self {
        self.state.insert(value);
        self
    }

    pub fn dev_mode(mut self, dev: bool) -> Self {
        self.dev = dev;
        self
    }

    pub fn project_root(mut self, root: PathBuf) -> Self {
        self.project_root = Some(root);
        self
    }

    pub fn build(self) -> App {
        // Start with the built-in commands so user overrides win.
        let mut ipc = builtins::register_all(IpcHandler::new());
        // Merge user handlers on top.
        for (cmd, f) in self.ipc.handlers_iter() {
            ipc = ipc.register_raw(cmd, f);
        }
        // Apply capabilities from the config.
        ipc = ipc.with_capabilities(self.config.capabilities.clone());
        // Let plugins contribute in order.
        for p in &self.plugins {
            ipc = p.register(ipc);
        }
        App {
            config: self.config,
            ipc: Arc::new(ipc),
            plugins: self.plugins,
            state: self.state,
            dev: self.dev,
            project_root: self.project_root,
        }
    }

    pub fn run(self) -> Result<()> {
        self.build().run()
    }
}

pub struct App {
    config: RizonetConfig,
    ipc: Arc<IpcHandler>,
    plugins: Vec<Box<dyn Plugin>>,
    state: StateMap,
    dev: bool,
    project_root: Option<PathBuf>,
}

impl App {
    pub fn run(self) -> Result<()> {
        // Resolve runtime (may download + extract a pinned runtime).
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let resolved: ResolvedRuntime = rt.block_on(runtime::resolve(
            &self.config.runtime,
            &self.config.app.identifier,
        ))?;
        tracing::info!(
            kind = ?resolved.kind,
            version = ?resolved.detected_version,
            "runtime resolved"
        );

        // Apply platform-specific runtime bindings before creating the webview.
        apply_runtime(&resolved);

        let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
        let proxy = event_loop.create_proxy();

        // Publish the global AppHandle so any code path can emit events etc.
        let app_handle = AppHandle::new(proxy, self.state.clone());
        let _ = APP_HANDLE.set(app_handle.clone());

        let win_cfg = &self.config.window;
        let window = WindowBuilder::new()
            .with_title(&win_cfg.title)
            .with_inner_size(LogicalSize::new(win_cfg.width, win_cfg.height))
            .with_resizable(win_cfg.resizable)
            .with_fullscreen(if win_cfg.fullscreen {
                Some(tao::window::Fullscreen::Borderless(None))
            } else {
                None
            })
            .with_maximized(!win_cfg.fullscreen && win_cfg.maximized)
            .with_decorations(win_cfg.decorations)
            .with_transparent(win_cfg.transparent)
            .build(&event_loop)?;

        let (url, assets_root) = self.resolve_start()?;
        tracing::info!(%url, ?assets_root, "loading");

        // Shared slot that the IPC handler will use to call back into the webview.
        // The handler runs on the same thread as the webview, so `Rc<RefCell<...>>`
        // is sufficient (and wry's ipc_handler does NOT require `Send`).
        let webview_slot: Rc<RefCell<Option<WebView>>> = Rc::new(RefCell::new(None));

        let ipc_for_handler = self.ipc.clone();
        let slot_for_handler = webview_slot.clone();

        let mut builder = WebViewBuilder::new(&window)
            .with_url(&url)
            .with_transparent(win_cfg.transparent)
            .with_initialization_script(ipc::BRIDGE_SCRIPT);

        // Register the custom asset protocol if we resolved a local asset root.
        // In development mode the page may live at an external `dev_url`, so
        // the handler is only useful when we have a local `dist/` directory.
        if let Some(root) = assets_root.clone() {
            let handler_root = Arc::new(root);
            builder = builder.with_custom_protocol(ASSET_SCHEME.into(), move |req| {
                serve_asset(&handler_root, req)
            });
        }

        let builder = builder.with_ipc_handler(move |req| {
                let body = req.body();
                let invoke: Invoke = match serde_json::from_str(body) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(?e, body, "invalid IPC message");
                        return;
                    }
                };
                let resp = ipc_for_handler.dispatch(invoke);
                let value_json = match &resp.data {
                    Some(v) => serde_json::to_string(v).unwrap_or_else(|_| "null".into()),
                    None => {
                        let err = resp.error.clone().unwrap_or_default();
                        serde_json::to_string(&err).unwrap_or_else(|_| "\"\"".into())
                    }
                };
                let script = format!(
                    "window.__RIZONET_RESOLVE__({}, {}, {});",
                    resp.id, resp.ok, value_json
                );
                if let Some(wv) = slot_for_handler.borrow().as_ref() {
                    if let Err(e) = wv.evaluate_script(&script) {
                        tracing::warn!(?e, "failed to evaluate IPC response");
                    }
                } else {
                    tracing::warn!("webview not ready when IPC response was produced");
                }
            });

        let webview = builder.build()?;
        *webview_slot.borrow_mut() = Some(webview);

        // Plugin on_start hooks.
        for p in &self.plugins {
            if let Err(e) = p.on_start() {
                tracing::error!(plugin = p.name(), ?e, "plugin on_start failed");
            }
        }

        let plugins = self.plugins;
        let webview_for_loop = webview_slot;
        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Wait;
            match event {
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => {
                    for p in &plugins {
                        p.on_exit();
                    }
                    *control_flow = ControlFlow::Exit;
                }
                Event::UserEvent(ue) => {
                    handle_user_event(ue, &window, &webview_for_loop, control_flow);
                }
                _ => {}
            }
        });
    }

    /// Decide which URL the webview should open and, if local, where the
    /// asset protocol handler should serve files from.
    fn resolve_start(&self) -> Result<(String, Option<PathBuf>)> {
        if self.dev {
            if let Some(u) = &self.config.app.dev_url {
                return Ok((u.clone(), None));
            }
        }
        let root = self
            .project_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let assets = root.join(&self.config.app.dist_dir);
        let url = format!("{ASSET_SCHEME}://{ASSET_HOST}/index.html");
        Ok((url, Some(assets)))
    }
}

/// Custom protocol handler: serve files from the asset root with the right
/// `Content-Type` headers. Rejects any request that tries to escape the
/// configured root via `..` segments.
fn serve_asset(root: &Path, req: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = req.uri().path();
    // Trim leading "/" and normalise.
    let rel = path.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };

    // Reject path traversal.
    for component in rel.split('/') {
        if component == ".." || component.starts_with('.') && component.len() > 1 && component != ".well-known" {
            // Allow plain dotfiles is fine in practice, but `..` is always forbidden.
        }
        if component == ".." {
            return Response::builder()
                .status(403)
                .body(Cow::Borrowed(&b"forbidden"[..]))
                .unwrap();
        }
    }

    let full = root.join(rel);
    match std::fs::read(&full) {
        Ok(bytes) => Response::builder()
            .status(200)
            .header("Content-Type", mime_for(&full))
            .header("Access-Control-Allow-Origin", "*")
            .body(Cow::Owned(bytes))
            .unwrap(),
        Err(e) => {
            tracing::warn!(path = %full.display(), ?e, "asset not found");
            Response::builder()
                .status(404)
                .header("Content-Type", "text/plain; charset=utf-8")
                .body(Cow::Owned(format!("not found: {}", rel).into_bytes()))
                .unwrap()
        }
    }
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()) {
        Some(ext) => match ext.as_str() {
            "html" | "htm" => "text/html; charset=utf-8",
            "js" | "mjs" => "application/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "json" => "application/json; charset=utf-8",
            "map" => "application/json; charset=utf-8",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            "webp" => "image/webp",
            "ico" => "image/x-icon",
            "wasm" => "application/wasm",
            "woff" => "font/woff",
            "woff2" => "font/woff2",
            "ttf" => "font/ttf",
            "otf" => "font/otf",
            "txt" | "md" => "text/plain; charset=utf-8",
            _ => "application/octet-stream",
        },
        None => "application/octet-stream",
    }
}

fn handle_user_event(
    event: UserEvent,
    window: &tao::window::Window,
    webview_slot: &Rc<RefCell<Option<WebView>>>,
    control_flow: &mut ControlFlow,
) {
    match event {
        UserEvent::EvalJs(script) => {
            if let Some(wv) = webview_slot.borrow().as_ref() {
                if let Err(e) = wv.evaluate_script(&script) {
                    tracing::warn!(?e, "eval_js failed");
                }
            }
        }
        UserEvent::Window(op) => match op {
            WindowOp::Show(v) => window.set_visible(v),
            WindowOp::Minimize(v) => window.set_minimized(v),
            WindowOp::Maximize(v) => window.set_maximized(v),
            WindowOp::Focus => window.set_focus(),
            WindowOp::SetTitle(t) => window.set_title(&t),
            WindowOp::Close => *control_flow = ControlFlow::Exit,
        },
        UserEvent::Devtools(open) => {
            if let Some(wv) = webview_slot.borrow().as_ref() {
                if open {
                    wv.open_devtools();
                } else {
                    wv.close_devtools();
                }
            }
        }
        UserEvent::Exit => *control_flow = ControlFlow::Exit,
    }
}

/// Apply platform-specific environment tweaks so that wry picks up the
/// bundled runtime when one is present.
fn apply_runtime(resolved: &ResolvedRuntime) {
    #[cfg(target_os = "windows")]
    {
        if let Some(folder) = &resolved.browser_executable_folder {
            tracing::info!(
                path = %folder.display(),
                "using bundled WebView2 (WEBVIEW2_BROWSER_EXECUTABLE_FOLDER)"
            );
            std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", folder);

            if let Some(runtime_root) = &resolved.runtime_root {
                let user_data = runtime_root.join("user-data");
                let _ = std::fs::create_dir_all(&user_data);
                std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", user_data);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = resolved;
    }
}
