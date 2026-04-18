//! Rizonet Studio — a GUI packager that turns any web folder into a
//! standalone native Rizonet binary.
//!
//! This binary is fully self-contained: the UI (HTML/CSS/JS) and the
//! configuration are embedded at compile time via `include_dir!`.
//! When launched the assets are extracted into a temporary folder so
//! that the webview can serve them through `file://` URLs.
//!
//! On Windows, no console window is spawned for release builds thanks
//! to `#![windows_subsystem = "windows"]` below.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::path::PathBuf;
use std::thread;

use include_dir::{include_dir, Dir};
use rizonet_core::acl::CapabilitiesConfig;
use rizonet_core::config::{AppConfig, BuildConfig, RuntimeConfig, WindowConfig};
use rizonet_core::{AppBuilder, IpcHandler, Result, RizonetConfig, RizonetError};
use rizonet_studio::builder::{self, BuildArgs};
use serde_json::json;

/// All web assets shipped inside the Studio binary.
static DIST: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../dist");

/// Extract the embedded web assets into a fresh temporary folder and return
/// its project root (the parent that contains a `dist/` directory).
fn extract_ui() -> Result<PathBuf> {
    let root = std::env::temp_dir().join(format!(
        "rizonet-studio-{}",
        std::process::id()
    ));
    // Start clean so stale files from a previous crash do not leak through.
    let _ = std::fs::remove_dir_all(&root);
    let dist_dir = root.join("dist");
    std::fs::create_dir_all(&dist_dir)?;
    DIST.extract(&dist_dir).map_err(|e| {
        RizonetError::Other(format!("failed to extract embedded UI: {e}"))
    })?;
    Ok(root)
}

/// Build the Studio's Rizonet config entirely in-code so the binary does not
/// need to load any external JSON.
fn studio_config() -> RizonetConfig {
    RizonetConfig {
        app: AppConfig {
            name: "Rizonet Studio".into(),
            identifier: "net.rizonet.studio".into(),
            dev_url: None,
            dist_dir: "dist".into(),
        },
        window: WindowConfig {
            title: "Rizonet Studio".into(),
            width: 980.0,
            height: 720.0,
            resizable: true,
            fullscreen: false,
            maximized: false,
            decorations: true,
            transparent: false,
        },
        runtime: RuntimeConfig::default(),
        build: BuildConfig::default(),
        capabilities: CapabilitiesConfig {
            allow: [
                "studio:*",
                "dialog:*",
                "shell:open",
                "path:*",
                "clipboard:*",
                "window:*",
                "devtools:*",
                "event:emit",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            deny: vec![],
        },
    }
}

fn install_crash_logging() {
    use std::fs::OpenOptions;
    use std::io::Write;

    let log_path = std::env::temp_dir().join("rizonet-studio.log");

    // Stream `tracing` output to the log file so we retain info/warn/error
    // lines even though the binary is a GUI subsystem app.
    if let Ok(file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug")),
            )
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .try_init();
    }

    // Catch every panic (including panics on worker threads).
    let log_for_panic = log_path.clone();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_for_panic)
        {
            let _ = writeln!(
                f,
                "\n[{}] PANIC: {info}\nbacktrace:\n{}",
                chrono_like_now(),
                std::backtrace::Backtrace::force_capture()
            );
        }
    }));
}

/// Tiny helper that formats the current time without pulling in `chrono`.
fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix={secs}")
}

fn main() -> Result<()> {
    install_crash_logging();
    tracing::info!("rizonet-studio starting");

    let project_root = extract_ui()?;
    let cfg = studio_config();

    let ipc = IpcHandler::new().register("studio:build", |payload| {
        let args: BuildArgs = serde_json::from_value(payload)?;
        // Run the long-running build on a worker thread. Progress streams
        // back through events so the GUI stays responsive.
        thread::spawn(move || {
            let h = rizonet_core::handle().clone();
            let h_for_log = h.clone();
            let logger: builder::Logger = Box::new(move |stream, line| {
                let _ = h_for_log.emit(
                    "studio:log",
                    json!({ "stream": stream, "line": line }),
                );
            });
            match builder::run(args, logger) {
                Ok(out) => {
                    let _ = h.emit(
                        "studio:done",
                        json!({
                            "binary": out.binary.display().to_string(),
                            "dist": out.dist.display().to_string(),
                            "config": out.config.display().to_string(),
                        }),
                    );
                }
                Err(e) => {
                    tracing::error!(?e, "studio build failed");
                    let _ = h.emit("studio:error", json!({ "message": e.to_string() }));
                }
            }
        });
        Ok(json!({ "accepted": true }))
    });

    AppBuilder::new(cfg)
        .ipc(ipc)
        .project_root(project_root)
        .run()
}
