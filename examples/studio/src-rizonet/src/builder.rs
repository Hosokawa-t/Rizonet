//! The packaging pipeline used by Rizonet Studio. Factored out of `main`
//! so it can be exercised headlessly by integration tests.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use directories::ProjectDirs;
use rizonet_core::{Result, RizonetError};
use serde::Deserialize;
use serde_json::json;

/// Absolute path to the Rizonet workspace root, captured at compile time
/// so generated projects can depend on `rizonet-core` via a local path.
pub const WORKSPACE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../..");

#[derive(Debug, Clone, Deserialize)]
pub struct BuildArgs {
    pub source_dir: String,
    pub output_dir: String,
    pub name: String,
    pub identifier: String,
    pub title: String,
    pub mode: String,
    /// One of `windowed`, `maximized`, `fullscreen`.
    #[serde(default = "default_window_mode")]
    pub window_mode: String,
    pub width: f64,
    pub height: f64,
    #[serde(default = "default_true")]
    pub resizable: bool,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub make_installer: bool,
}

fn default_window_mode() -> String {
    "windowed".into()
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
pub struct BuildOutcome {
    pub binary: PathBuf,
    pub dist: PathBuf,
    pub config: PathBuf,
    pub installer: Option<PathBuf>,
}

/// Callback invoked for every line of build log. `stream` is either
/// `"stdout"`, `"stderr"`, or `"info"` for framework messages.
pub type Logger = Box<dyn Fn(&str, &str) + Send + Sync>;

pub fn sanitize_crate_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
        s.insert(0, 'a');
    }
    if s.is_empty() {
        s.push_str("app");
    }
    s
}

fn cache_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("net", "rizonet", "studio")
        .ok_or_else(|| RizonetError::Other("cannot resolve cache dir".into()))?;
    let p = dirs.cache_dir().join("builds");
    fs::create_dir_all(&p)?;
    Ok(p)
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if ty.is_file() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Stream a child process's stdout and stderr line-by-line through the logger
/// and block until completion. Returns an error on non-zero exit.
fn stream_cargo_build(
    build_root: &Path,
    log: &std::sync::Arc<Logger>,
    action: &str,
) -> Result<()> {
    log("info", &format!("▶ cargo build --release ({action}) in {}", build_root.display()));
    let mut child = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(build_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| RizonetError::Other(format!("failed to spawn cargo: {e}")))?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let log_out = log.clone();
    let out_thread = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|r| r.ok()) {
            log_out("stdout", &line);
        }
    });
    let log_err = log.clone();
    let err_thread = thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(|r| r.ok()) {
            log_err("stderr", &line);
        }
    });

    let status = child
        .wait()
        .map_err(|e| RizonetError::Other(format!("cargo wait failed: {e}")))?;
    let _ = out_thread.join();
    let _ = err_thread.join();
    if !status.success() {
        return Err(RizonetError::Other(format!(
            "cargo exited with status {status}"
        )));
    }
    Ok(())
}

/// Run the full packaging pipeline.
pub fn run(args: BuildArgs, log: Logger) -> Result<BuildOutcome> {
    let source = PathBuf::from(&args.source_dir);
    if !source.is_dir() {
        return Err(RizonetError::Other(format!(
            "source folder does not exist: {}",
            source.display()
        )));
    }
    if !source.join("index.html").is_file() {
        return Err(RizonetError::Other(
            "source folder must contain an index.html".into(),
        ));
    }
    let output = PathBuf::from(&args.output_dir);
    fs::create_dir_all(&output)?;

    let crate_name = sanitize_crate_name(&args.name);
    log("info", &format!("crate name: {crate_name}"));

    let build_root = cache_dir()?.join(&crate_name);
    fs::create_dir_all(build_root.join("src"))?;

    let workspace = WORKSPACE_ROOT.replace('\\', "/");
    let cargo_toml = format!(
        r#"[package]
name = "{crate_name}"
version = "0.1.0"
edition = "2021"

# Isolated workspace so we do not inherit the Rizonet repository's workspace.
[workspace]

[[bin]]
name = "{crate_name}"
path = "src/main.rs"

[dependencies]
rizonet-core = {{ path = "{workspace}/crates/rizonet-core" }}
tracing-subscriber = {{ version = "0.3", features = ["env-filter"] }}

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "unwind"
"#
    );
    fs::write(build_root.join("Cargo.toml"), cargo_toml)?;

    let main_rs = r##"#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use rizonet_core::{AppBuilder, RizonetConfig, Result};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load the config that sits beside the executable (single-folder layout).
    let exe = std::env::current_exe().expect("current_exe");
    let root = exe.parent().expect("exe parent").to_path_buf();
    let cfg = RizonetConfig::load(root.join("rizonet.config.json"))
        .expect("failed to load rizonet.config.json next to the executable");

    AppBuilder::new(cfg).project_root(root).run()
}
"##;
    fs::write(build_root.join("src").join("main.rs"), main_rs)?;

    // ── Build the end-user app ───────────────────────────────────────
    let log = std::sync::Arc::new(log);
    stream_cargo_build(&build_root, &log, "user app")?;

    let exe_suffix = if cfg!(target_os = "windows") { ".exe" } else { "" };
    let built_exe = build_root
        .join("target")
        .join("release")
        .join(format!("{crate_name}{exe_suffix}"));
    if !built_exe.is_file() {
        return Err(RizonetError::Other(format!(
            "expected binary not found: {}",
            built_exe.display()
        )));
    }
    let out_exe = output.join(format!("{crate_name}{exe_suffix}"));
    fs::copy(&built_exe, &out_exe)?;
    log("info", &format!("copied → {}", out_exe.display()));

    let out_dist = output.join("dist");
    if out_dist.exists() {
        fs::remove_dir_all(&out_dist)?;
    }
    copy_dir_all(&source, &out_dist)
        .map_err(|e| RizonetError::Other(format!("failed to copy web assets: {e}")))?;
    log(
        "info",
        &format!("copied web assets → {}", out_dist.display()),
    );

    // ── Window mode → fullscreen/maximized flags ─────────────────────
    let (fullscreen, maximized) = match args.window_mode.as_str() {
        "fullscreen" => (true, false),
        "maximized" => (false, true),
        _ => (false, false),
    };
    let mode = match args.mode.as_str() {
        "system" | "strict" => args.mode.as_str(),
        _ => "auto",
    };
    let config_json = json!({
        "app": {
            "name": args.name,
            "identifier": args.identifier,
            "dist_dir": "dist"
        },
        "window": {
            "title": args.title,
            "width": args.width,
            "height": args.height,
            "resizable": args.resizable,
            "fullscreen": fullscreen,
            "maximized": maximized
        },
        "runtime": { "mode": mode },
        "capabilities": { "allow": args.allow }
    });
    let out_config = output.join("rizonet.config.json");
    fs::write(&out_config, serde_json::to_string_pretty(&config_json)?)?;
    log("info", "wrote rizonet.config.json");

    // ── Optional: build a self-extracting Windows installer ──────────
    let installer = if args.make_installer && cfg!(target_os = "windows") {
        match build_installer(&args, &crate_name, &output, &log) {
            Ok(p) => Some(p),
            Err(e) => {
                log("stderr", &format!("installer build failed: {e}"));
                None
            }
        }
    } else {
        None
    };

    Ok(BuildOutcome {
        binary: out_exe,
        dist: out_dist,
        config: out_config,
        installer,
    })
}

/// Build a self-extracting installer that bundles `output_dir` and, on
/// first run, extracts everything into `%LOCALAPPDATA%\Programs\<name>\`,
/// creates a Start Menu shortcut, and writes an uninstall helper.
///
/// Implemented as a second, isolated cargo project so it can use
/// `include_dir!("<output_dir>")` to statically embed the produced app.
fn build_installer(
    args: &BuildArgs,
    crate_name: &str,
    payload_dir: &Path,
    log: &std::sync::Arc<Logger>,
) -> Result<PathBuf> {
    let installer_crate = format!("{crate_name}_setup");
    let build_root = cache_dir()?.join(&installer_crate);
    fs::create_dir_all(build_root.join("src"))?;

    // Absolute, forward-slashed payload path for include_dir!.
    let payload_abs = payload_dir
        .canonicalize()
        .unwrap_or_else(|_| payload_dir.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");

    let cargo_toml = format!(
        r#"[package]
name = "{installer_crate}"
version = "0.1.0"
edition = "2021"

[workspace]

[[bin]]
name = "{installer_crate}"
path = "src/main.rs"

[dependencies]
include_dir = "0.7"
rfd = "0.15"
directories = "5"

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "unwind"
"#
    );
    fs::write(build_root.join("Cargo.toml"), cargo_toml)?;

    // Escape user-facing strings so they round-trip through the Rust source.
    let app_name_rs = escape_rust_str(&args.name);
    let identifier_rs = escape_rust_str(&args.identifier);
    let exe_name_rs = escape_rust_str(&format!("{crate_name}.exe"));
    let payload_rs = escape_rust_str(&payload_abs);

    let main_rs = format!(
        r##"#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

//! Auto-generated installer for "{app_name_display}".
//!
//! Embeds the packaged app and, when run by an end user, extracts it to
//! `%LOCALAPPDATA%\\Programs\\<identifier>\\`, then creates a Start Menu
//! shortcut. A simple GUI dialog drives the flow.

use std::fs;
use std::io::Write;
use std::path::{{Path, PathBuf}};
use std::process::Command;

use include_dir::{{include_dir, Dir}};

const APP_NAME: &str = "{app_name_rs}";
const IDENTIFIER: &str = "{identifier_rs}";
const EXE_NAME: &str = "{exe_name_rs}";
static PAYLOAD: Dir<'_> = include_dir!("{payload_rs}");

fn install_dir() -> PathBuf {{
    let base = directories::BaseDirs::new()
        .map(|b| b.data_local_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir());
    base.join("Programs").join(IDENTIFIER)
}}

fn start_menu_dir() -> Option<PathBuf> {{
    directories::BaseDirs::new().map(|b| {{
        b.data_dir()
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
    }})
}}

fn extract_all(dst: &Path) -> std::io::Result<()> {{
    fs::create_dir_all(dst)?;
    PAYLOAD.extract(dst)
}}

fn create_shortcut(target_exe: &Path, shortcut_path: &Path) -> std::io::Result<()> {{
    // Use WScript.Shell via PowerShell to create a proper .lnk file.
    let script = format!(
        "$s = (New-Object -COM WScript.Shell).CreateShortcut('{{}}');\
         $s.TargetPath = '{{}}';\
         $s.WorkingDirectory = '{{}}';\
         $s.Save();",
        shortcut_path.display(),
        target_exe.display(),
        target_exe.parent().unwrap_or_else(|| Path::new(".")).display(),
    );
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()?;
    Ok(())
}}

fn write_uninstaller(install_dir: &Path) -> std::io::Result<()> {{
    // A tiny .cmd that removes the install directory and the Start Menu entry.
    let script = format!(
        "@echo off\r\n\
         set DIR={{}}\r\n\
         echo Uninstalling %DIR%\r\n\
         del /F /Q \"%APPDATA%\\Microsoft\\Windows\\Start Menu\\Programs\\{{}}.lnk\" 2>nul\r\n\
         timeout /t 1 >nul\r\n\
         rmdir /S /Q \"%DIR%\"\r\n\
         echo Done.\r\n\
         pause\r\n",
        install_dir.display(),
        APP_NAME,
    );
    let mut f = fs::File::create(install_dir.join("uninstall.cmd"))?;
    f.write_all(script.as_bytes())
}}

fn run_install() -> Result<PathBuf, String> {{
    let dir = install_dir();
    let _ = fs::remove_dir_all(&dir); // fresh
    extract_all(&dir).map_err(|e| format!("extract failed: {{e}}"))?;
    let exe = dir.join(EXE_NAME);

    if let Some(menu) = start_menu_dir() {{
        let _ = fs::create_dir_all(&menu);
        let lnk = menu.join(format!("{{APP_NAME}}.lnk"));
        let _ = create_shortcut(&exe, &lnk);
    }}
    let _ = write_uninstaller(&dir);
    Ok(exe)
}}

fn main() {{
    let msg = format!(
        "This will install {{APP_NAME}} to:\n\n{{}}\n\nProceed?",
        install_dir().display()
    );
    let proceed = rfd::MessageDialog::new()
        .set_title(&format!("Install {{APP_NAME}}"))
        .set_description(&msg)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();
    if !matches!(proceed, rfd::MessageDialogResult::Yes) {{
        return;
    }}

    match run_install() {{
        Ok(exe) => {{
            let launch = rfd::MessageDialog::new()
                .set_title(&format!("{{APP_NAME}} installed"))
                .set_description(&format!(
                    "Installed to:\n{{}}\n\nA Start Menu shortcut has been created.\n\nLaunch now?",
                    exe.parent().unwrap().display()
                ))
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            if matches!(launch, rfd::MessageDialogResult::Yes) {{
                let _ = Command::new(&exe).spawn();
            }}
        }}
        Err(e) => {{
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title(&format!("{{APP_NAME}} installer"))
                .set_description(&format!("Install failed: {{e}}"))
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }}
    }}
}}
"##,
        app_name_display = args.name.replace('"', "\\\""),
        app_name_rs = app_name_rs,
        identifier_rs = identifier_rs,
        exe_name_rs = exe_name_rs,
        payload_rs = payload_rs,
    );
    fs::write(build_root.join("src").join("main.rs"), main_rs)?;

    stream_cargo_build(&build_root, log, "installer")?;

    let exe_suffix = if cfg!(target_os = "windows") { ".exe" } else { "" };
    let built = build_root
        .join("target")
        .join("release")
        .join(format!("{installer_crate}{exe_suffix}"));
    if !built.is_file() {
        return Err(RizonetError::Other(format!(
            "installer binary not found: {}",
            built.display()
        )));
    }
    let out = payload_dir.join(format!("{crate_name}-Setup{exe_suffix}"));
    fs::copy(&built, &out)?;
    log("info", &format!("copied installer → {}", out.display()));
    Ok(out)
}

/// Escape a string so it can be embedded as a Rust string literal.
fn escape_rust_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
