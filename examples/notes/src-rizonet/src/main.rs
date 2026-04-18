#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

//! Rizonet Notes — example application.
//!
//! Demonstrates:
//! - Mounting a webview app from `dist/` via Rizonet.
//! - Exposing filesystem operations to the frontend through IPC.
//! - Scoping all operations to a per-OS user data directory so that the
//!   web side cannot escape it (no path traversal).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use directories::ProjectDirs;
use rizonet_core::{AppBuilder, IpcHandler, Result, RizonetConfig, RizonetError};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Cached notes directory (`<user data>/rizonet/notes/notes`). Resolved once on startup.
static NOTES_DIR: OnceLock<PathBuf> = OnceLock::new();

fn notes_dir() -> &'static Path {
    NOTES_DIR.get().expect("notes dir not initialized").as_path()
}

fn init_notes_dir() -> anyhow::Result<PathBuf> {
    let dirs = ProjectDirs::from("net", "rizonet", "notes")
        .ok_or_else(|| anyhow::anyhow!("cannot resolve project dirs"))?;
    let dir = dirs.data_dir().join("notes");
    fs::create_dir_all(&dir)?;
    // Seed a welcome note on first run so the UI isn't empty.
    let welcome = dir.join("Welcome.md");
    if !welcome.exists() {
        fs::write(
            &welcome,
            "# Welcome to Rizonet Notes\n\n\
             This app is a tiny demo showing off the Rizonet framework.\n\n\
             - Hit **+** to create a note.\n\
             - Press **Ctrl/Cmd+S** to save.\n\
             - Markdown is rendered live on the right.\n\n\
             ```rust\nfn main() {\n    println!(\"Hello from Rust!\");\n}\n```\n",
        )?;
    }
    Ok(dir)
}

#[derive(Debug, Serialize)]
struct NoteSummary {
    name: String,
    size: u64,
    modified_unix: u64,
}

#[derive(Debug, Deserialize)]
struct NameArgs {
    name: String,
}

#[derive(Debug, Deserialize)]
struct WriteArgs {
    name: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct RenameArgs {
    from: String,
    to: String,
}

/// Reject any file name that tries to escape the notes directory or use a
/// non-markdown extension. We only allow `[A-Za-z0-9 ._-]` and require
/// the `.md` suffix.
fn sanitize_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(RizonetError::Other("empty name".into()));
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        return Err(RizonetError::Other(format!("illegal path: {trimmed}")));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-'))
    {
        return Err(RizonetError::Other(format!(
            "illegal characters in name: {trimmed}"
        )));
    }
    let name = if trimmed.to_ascii_lowercase().ends_with(".md") {
        trimmed.to_string()
    } else {
        format!("{trimmed}.md")
    };
    Ok(name)
}

fn note_path(name: &str) -> Result<PathBuf> {
    let safe = sanitize_name(name)?;
    Ok(notes_dir().join(safe))
}

fn list_notes() -> Result<Vec<NoteSummary>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(notes_dir())? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.to_ascii_lowercase().ends_with(".md") => n.to_string(),
            _ => continue,
        };
        let meta = entry.metadata()?;
        let modified_unix = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(NoteSummary {
            name,
            size: meta.len(),
            modified_unix,
        });
    }
    // Newest first.
    out.sort_by(|a, b| b.modified_unix.cmp(&a.modified_unix));
    Ok(out)
}

fn read_note(name: &str) -> Result<String> {
    let path = note_path(name)?;
    let data = fs::read_to_string(&path)?;
    Ok(data)
}

fn write_note(name: &str, content: &str) -> Result<()> {
    let path = note_path(name)?;
    fs::write(&path, content)?;
    Ok(())
}

fn delete_note(name: &str) -> Result<()> {
    let path = note_path(name)?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

fn rename_note(from: &str, to: &str) -> Result<()> {
    let src = note_path(from)?;
    let dst = note_path(to)?;
    if dst.exists() {
        return Err(RizonetError::Other(format!(
            "target already exists: {}",
            dst.display()
        )));
    }
    fs::rename(&src, &dst)?;
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let dir = init_notes_dir().expect("failed to initialize notes directory");
    let _ = NOTES_DIR.set(dir.clone());
    tracing::info!(path = %dir.display(), "notes dir ready");

    // When running with `cargo run` from the workspace root, the current
    // directory is the workspace root; discover the config starting from
    // the example directory so we get a stable dist_dir.
    let example_root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));
    let (cfg, cfg_path) = RizonetConfig::discover(&example_root)
        .expect("rizonet.config.json not found");
    let project_root = cfg_path.parent().unwrap().to_path_buf();

    let ipc = IpcHandler::new()
        .register("notes:list", |_| {
            let notes = list_notes()?;
            Ok(serde_json::to_value(notes)?)
        })
        .register("notes:read", |payload| {
            let args: NameArgs = serde_json::from_value(payload)?;
            Ok(json!(read_note(&args.name)?))
        })
        .register("notes:write", |payload| {
            let args: WriteArgs = serde_json::from_value(payload)?;
            write_note(&args.name, &args.content)?;
            // Broadcast a saved event so other parts of the UI can react.
            if let Some(h) = rizonet_core::try_handle() {
                let _ = h.emit("notes:saved", json!({ "name": args.name }));
            }
            Ok(json!({ "ok": true }))
        })
        .register("notes:delete", |payload| {
            let args: NameArgs = serde_json::from_value(payload)?;
            delete_note(&args.name)?;
            Ok(json!({ "ok": true }))
        })
        .register("notes:rename", |payload| {
            let args: RenameArgs = serde_json::from_value(payload)?;
            rename_note(&args.from, &args.to)?;
            Ok(json!({ "ok": true }))
        })
        .register("notes:reveal_dir", |_| {
            Ok(json!(notes_dir().display().to_string()))
        });

    let dev = std::env::var("RIZONET_DEV").is_ok();

    AppBuilder::new(cfg)
        .ipc(ipc)
        .dev_mode(dev)
        .project_root(project_root)
        .run()
}
