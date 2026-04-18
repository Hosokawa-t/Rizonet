#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use rizonet_core::{AppBuilder, IpcHandler, Plugin, Result, RizonetConfig};
use serde_json::json;

/// Example plugin registering a `clock:now` command.
struct ClockPlugin;

impl Plugin for ClockPlugin {
    fn name(&self) -> &str {
        "clock"
    }
    fn register(&self, ipc: IpcHandler) -> IpcHandler {
        ipc.register("clock:now", |_| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Ok(json!({ "unix": now }))
        })
    }
    fn on_start(&self) -> Result<()> {
        tracing::info!("clock plugin started");
        Ok(())
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let (cfg, cfg_path) =
        RizonetConfig::discover(std::env::current_dir().expect("cwd"))
            .expect("rizonet.config.json not found");
    let project_root = cfg_path.parent().unwrap().to_path_buf();

    let ipc = IpcHandler::new()
        .register("ping", |payload| {
            Ok(json!({
                "pong": true,
                "echo": payload,
                "runtime": "rizonet",
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
            }))
        })
        .register("app_version", |_| Ok(json!(env!("CARGO_PKG_VERSION"))))
        .register("platform_key", |_| {
            Ok(json!(rizonet_core::platform_key()))
        });

    let dev = std::env::var("RIZONET_DEV").is_ok();

    AppBuilder::new(cfg)
        .ipc(ipc)
        .plugin(ClockPlugin)
        .dev_mode(dev)
        .project_root(project_root)
        .run()
}
