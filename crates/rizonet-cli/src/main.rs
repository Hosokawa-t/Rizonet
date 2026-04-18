//! The `rizonet` command-line interface.
//!
//! Subcommands:
//! - `rizonet new <name>`       : Scaffold a new project.
//! - `rizonet dev`              : Run the current project in dev mode.
//! - `rizonet build`            : Build the current project in release mode.
//! - `rizonet info`             : Print environment diagnostics.
//! - `rizonet update-check ...` : Check a remote manifest for updates.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(name = "rizonet", version, about = "Rizonet — the ultimate desktop framework")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold a new Rizonet project.
    New {
        name: String,
        #[arg(long, default_value = "vanilla")]
        template: String,
    },
    /// Run the app in development mode.
    Dev,
    /// Build the app in release mode.
    Build,
    /// Print environment diagnostics.
    Info,
    /// Check a remote manifest for an available update.
    UpdateCheck {
        #[arg(long)]
        manifest: String,
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        current: String,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::New { name, template } => new_project(&name, &template),
        Cmd::Dev => dev(),
        Cmd::Build => build(),
        Cmd::Info => info(),
        Cmd::UpdateCheck { manifest, current } => update_check(&manifest, &current),
    }
}

fn update_check(manifest_url: &str, current: &str) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let info = rt
        .block_on(rizonet_core::updater::check(manifest_url, current))
        .map_err(to_anyhow)?;
    match info {
        None => println!("up to date (current: {current})"),
        Some(info) => {
            println!(
                "update available: {} -> {} ({})",
                current, info.manifest.version, info.platform
            );
            if !info.manifest.notes.is_empty() {
                println!("\nnotes:\n{}", info.manifest.notes);
            }
            println!("\nartifact: {}", info.artifact.url);
        }
    }
    Ok(())
}

fn new_project(name: &str, template: &str) -> Result<()> {
    if template != "vanilla" {
        anyhow::bail!("unknown template: {template}. available: vanilla");
    }
    let root = PathBuf::from(name);
    if root.exists() {
        anyhow::bail!("{:?} already exists", root);
    }
    std::fs::create_dir_all(root.join("dist"))?;
    std::fs::create_dir_all(root.join("src-rizonet/src"))?;

    std::fs::write(
        root.join("rizonet.config.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "app": {
                "name": name,
                "identifier": format!("net.rizonet.{}", sanitize_ident(name)),
                "dev_url": null,
                "dist_dir": "dist"
            },
            "window": {
                "title": name,
                "width": 1024,
                "height": 768
            },
            "runtime": {
                "mode": "auto"
            }
        }))?,
    )?;

    std::fs::write(root.join("dist/index.html"), TEMPLATE_INDEX_HTML)?;
    std::fs::write(root.join("src-rizonet/Cargo.toml"), app_cargo_toml(name))?;
    std::fs::write(root.join("src-rizonet/src/main.rs"), TEMPLATE_MAIN_RS)?;

    println!("✨ Rizonet project created at {:?}", root);
    println!("   next:");
    println!("     cd {name}");
    println!("     rizonet dev");
    Ok(())
}

fn sanitize_ident(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect()
}

fn dev() -> Result<()> {
    let (_cfg, cfg_path) =
        rizonet_core::RizonetConfig::discover(std::env::current_dir()?).map_err(to_anyhow)?;
    let root = cfg_path.parent().unwrap().to_path_buf();
    run_cargo(&root, &["run"], true)
}

fn build() -> Result<()> {
    let (_cfg, cfg_path) =
        rizonet_core::RizonetConfig::discover(std::env::current_dir()?).map_err(to_anyhow)?;
    let root = cfg_path.parent().unwrap().to_path_buf();
    run_cargo(&root, &["build", "--release"], false)
}

fn info() -> Result<()> {
    println!("Rizonet v{}", env!("CARGO_PKG_VERSION"));
    println!("OS: {}", std::env::consts::OS);
    println!("ARCH: {}", std::env::consts::ARCH);
    #[cfg(target_os = "windows")]
    println!("WebView2: (check `Get-AppxPackage *WebView*` in PowerShell)");
    Ok(())
}

fn run_cargo(project_root: &Path, args: &[&str], dev: bool) -> Result<()> {
    let app_crate = project_root.join("src-rizonet");
    if !app_crate.join("Cargo.toml").exists() {
        anyhow::bail!(
            "src-rizonet/Cargo.toml not found under {:?}. Did you run `rizonet new`?",
            project_root
        );
    }
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&app_crate).args(args);
    if dev {
        cmd.env("RIZONET_DEV", "1");
    }
    let status = cmd.status().with_context(|| "failed to spawn cargo")?;
    if !status.success() {
        anyhow::bail!("cargo exited with status {status}");
    }
    Ok(())
}

fn to_anyhow(e: rizonet_core::RizonetError) -> anyhow::Error {
    anyhow::anyhow!(e.to_string())
}

const TEMPLATE_INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>Rizonet</title>
  <style>
    :root { color-scheme: light dark; }
    body {
      font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
      margin: 0; padding: 2rem;
      display: grid; place-items: center; min-height: 100vh;
      background: linear-gradient(135deg, #0ea5e9, #8b5cf6);
      color: white;
    }
    .card {
      background: rgba(0,0,0,0.35);
      backdrop-filter: blur(12px);
      padding: 2rem 3rem; border-radius: 1rem;
      box-shadow: 0 10px 40px rgba(0,0,0,0.3);
      text-align: center;
    }
    h1 { margin: 0 0 .5rem; font-size: 2.5rem; }
    button {
      margin-top: 1rem; padding: .6rem 1.2rem; border: 0; border-radius: .5rem;
      background: white; color: #111; font-weight: 600; cursor: pointer;
    }
    pre { text-align: left; background: rgba(0,0,0,0.4); padding: 1rem; border-radius: .5rem; }
  </style>
</head>
<body>
  <div class="card">
    <h1>⚡ Rizonet</h1>
    <p>Lightweight, fast, and environment-independent &mdash; the ultimate desktop framework.</p>
    <button id="ping">ping Rust</button>
    <pre id="out">—</pre>
  </div>
  <script>
    document.getElementById('ping').addEventListener('click', async () => {
      try {
        const r = await window.__RIZONET__.invoke('ping', { from: 'web' });
        document.getElementById('out').textContent = JSON.stringify(r, null, 2);
      } catch (e) {
        document.getElementById('out').textContent = 'error: ' + e.message;
      }
    });
  </script>
</body>
</html>
"#;

fn app_cargo_toml(name: &str) -> String {
    format!(
        r#"[package]
name = "{ident}"
version = "0.1.0"
edition = "2021"

[dependencies]
rizonet-core = {{ path = "../../../crates/rizonet-core" }}
serde_json = "1"
tracing-subscriber = "0.3"
"#,
        ident = sanitize_ident(name)
    )
}

const TEMPLATE_MAIN_RS: &str = r##"use rizonet_core::{AppBuilder, IpcHandler, RizonetConfig};
use serde_json::json;

fn main() -> rizonet_core::Result<()> {
    tracing_subscriber::fmt().init();

    let (mut cfg, cfg_path) =
        RizonetConfig::discover(std::env::current_dir().unwrap()).unwrap();
    let project_root = cfg_path.parent().unwrap().to_path_buf();

    // When `dev_url` is not configured, dev mode falls back to loading
    // `dist/index.html` through a `file://` URL, which keeps the dev
    // loop trivial for pure-static frontends.
    let dev = std::env::var("RIZONET_DEV").is_ok();
    let _ = &mut cfg;

    let ipc = IpcHandler::new().register("ping", |payload| {
        Ok(json!({
            "pong": true,
            "echo": payload,
            "runtime": "rizonet"
        }))
    });

    AppBuilder::new(cfg)
        .ipc(ipc)
        .dev_mode(dev)
        .project_root(project_root)
        .run()
}
"##;
