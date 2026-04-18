//! The packaging pipeline used by Rizonet Studio. Factored out of `main`
//! so it can be exercised headlessly by integration tests.
//!
//! Pipeline overview:
//! 1. Validate the source web folder.
//! 2. (Optional) convert any image into a multi-resolution `.ico`.
//! 3. Scaffold an isolated cargo crate that depends on `rizonet-core`,
//!    writes a `build.rs` which embeds the `.ico` as a Win32 resource
//!    (via `winresource`), and runs `cargo build --release`.
//! 4. Copy the resulting binary, web assets and `rizonet.config.json`
//!    into the user's output folder.
//! 5. (Optional) scaffold a second crate — a full egui-based GUI
//!    installer — that embeds a zipped copy of the output folder and
//!    produces `<name>-Setup.exe`.

use std::fs;
use std::io::{BufRead, BufReader, Write};
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
    /// `windowed` | `maximized` | `fullscreen`.
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
    /// Optional path to an image of ANY format and ANY aspect ratio. It is
    /// letterboxed onto a transparent square canvas, converted to a
    /// multi-resolution `.ico`, and embedded into both the app and the
    /// installer EXE.
    #[serde(default)]
    pub icon_path: Option<String>,
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
    pub icon: Option<PathBuf>,
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

/// Zip `dir` recursively (stored + deflate) into `out_zip`.
fn zip_directory(dir: &Path, out_zip: &Path) -> Result<()> {
    use std::io::Read;
    use zip::write::SimpleFileOptions;
    let file = fs::File::create(out_zip)?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);
    fn walk(
        zw: &mut zip::ZipWriter<fs::File>,
        root: &Path,
        cur: &Path,
        opts: SimpleFileOptions,
    ) -> Result<()> {
        for entry in fs::read_dir(cur)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let p = entry.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if ty.is_dir() {
                zw.add_directory(&rel, opts)
                    .map_err(|e| RizonetError::Other(format!("zip mkdir: {e}")))?;
                walk(zw, root, &p, opts)?;
            } else if ty.is_file() {
                zw.start_file(&rel, opts)
                    .map_err(|e| RizonetError::Other(format!("zip start: {e}")))?;
                let mut f = fs::File::open(&p)?;
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                zw.write_all(&buf)?;
            }
        }
        Ok(())
    }
    walk(&mut zw, dir, dir, opts)?;
    zw.finish()
        .map_err(|e| RizonetError::Other(format!("zip finish: {e}")))?;
    Ok(())
}

/// Stream a child process's stdout and stderr line-by-line through the
/// logger and block until completion.
fn stream_cargo_build(
    build_root: &Path,
    log: &std::sync::Arc<Logger>,
    action: &str,
) -> Result<()> {
    log(
        "info",
        &format!("▶ cargo build --release ({action}) in {}", build_root.display()),
    );
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
    let log = std::sync::Arc::new(log);
    log("info", &format!("crate name: {crate_name}"));

    let build_root = cache_dir()?.join(&crate_name);
    fs::create_dir_all(build_root.join("src"))?;

    // ── Icon: convert user-supplied image to a multi-res .ico ────────
    let icon_ico_path: Option<PathBuf> = match args.icon_path.as_deref() {
        Some(p) if !p.is_empty() => {
            let src = PathBuf::from(p);
            if !src.is_file() {
                return Err(RizonetError::Other(format!(
                    "icon source not found: {}",
                    src.display()
                )));
            }
            log(
                "info",
                &format!("converting icon: {} → .ico (7 sizes)", src.display()),
            );
            let ico = build_root.join("icon.ico");
            rizonet_core::iconify::image_to_ico(&src, &ico)?;
            // Also drop a copy next to the final binary for convenience.
            let out_ico = output.join("icon.ico");
            let _ = fs::copy(&ico, &out_ico);
            Some(ico)
        }
        _ => None,
    };

    let workspace = WORKSPACE_ROOT.replace('\\', "/");
    let has_icon = icon_ico_path.is_some();
    let build_deps_section = if has_icon {
        r#"[build-dependencies]
winresource = "0.1"
"#
    } else {
        ""
    };
    let cargo_toml = format!(
        r#"[package]
name = "{crate_name}"
version = "0.1.0"
edition = "2021"
build = "build.rs"

# Isolated workspace so we do not inherit the Rizonet repository's workspace.
[workspace]

[[bin]]
name = "{crate_name}"
path = "src/main.rs"

[dependencies]
rizonet-core = {{ path = "{workspace}/crates/rizonet-core" }}
tracing-subscriber = {{ version = "0.3", features = ["env-filter"] }}

{build_deps_section}
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "unwind"
"#
    );
    fs::write(build_root.join("Cargo.toml"), cargo_toml)?;

    // build.rs: embed the icon as a Win32 resource on Windows, no-op elsewhere.
    let build_rs = if has_icon {
        r##"fn main() {
    println!("cargo:rerun-if-changed=icon.ico");
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icon.ico");
        if let Err(e) = res.compile() {
            eprintln!("warning: failed to embed icon: {e}");
        }
    }
}
"##
    } else {
        r##"fn main() {}
"##
    };
    fs::write(build_root.join("build.rs"), build_rs)?;

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

    let exe = std::env::current_exe().expect("current_exe");
    let root = exe.parent().expect("exe parent").to_path_buf();
    let cfg = RizonetConfig::load(root.join("rizonet.config.json"))
        .expect("failed to load rizonet.config.json next to the executable");

    AppBuilder::new(cfg).project_root(root).run()
}
"##;
    fs::write(build_root.join("src").join("main.rs"), main_rs)?;

    // ── Build the end-user app ───────────────────────────────────────
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

    let out_icon = if has_icon { Some(output.join("icon.ico")) } else { None };

    // ── Optional: build a full GUI Windows installer ─────────────────
    let installer = if args.make_installer && cfg!(target_os = "windows") {
        match build_installer(&args, &crate_name, &output, icon_ico_path.as_deref(), &log) {
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
        icon: out_icon,
        installer,
    })
}

/// Build a full egui-based GUI installer that bundles the produced app
/// folder as a compressed zip payload, extracts it at install time,
/// creates shortcuts, and registers the app in `Add/Remove Programs`.
/// The same binary doubles as uninstaller when run with `--uninstall`.
fn build_installer(
    args: &BuildArgs,
    crate_name: &str,
    payload_dir: &Path,
    icon_src: Option<&Path>,
    log: &std::sync::Arc<Logger>,
) -> Result<PathBuf> {
    let installer_crate = format!("{crate_name}_setup");
    let build_root = cache_dir()?.join(&installer_crate);
    fs::create_dir_all(build_root.join("src"))?;

    // 1. Zip the payload (the whole output folder, *excluding* a previously
    //    generated <name>-Setup.exe to avoid recursion).
    let staging = build_root.join("payload_stage");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    copy_dir_all(payload_dir, &staging)
        .map_err(|e| RizonetError::Other(format!("stage payload: {e}")))?;
    let prev_installer = staging.join(format!("{crate_name}-Setup.exe"));
    let _ = fs::remove_file(&prev_installer);
    let payload_zip = build_root.join("payload.zip");
    zip_directory(&staging, &payload_zip)?;
    log(
        "info",
        &format!(
            "payload zipped: {} ({} KB)",
            payload_zip.display(),
            fs::metadata(&payload_zip).map(|m| m.len() / 1024).unwrap_or(0)
        ),
    );

    // 2. Copy the icon into the installer crate (for window icon + EXE resource).
    let icon_dst = build_root.join("icon.ico");
    if let Some(src) = icon_src {
        let _ = fs::copy(src, &icon_dst);
    }
    let has_icon = icon_dst.is_file();

    // 3. Render the installer crate manifest.
    let build_deps = if has_icon {
        r#"[build-dependencies]
winresource = "0.1"
"#
    } else {
        ""
    };
    let cargo_toml = format!(
        r#"[package]
name = "{installer_crate}"
version = "0.1.0"
edition = "2021"
build = "build.rs"

[workspace]

[[bin]]
name = "{installer_crate}"
path = "src/main.rs"

[dependencies]
eframe = {{ version = "0.29", default-features = false, features = ["default_fonts", "glow", "wayland", "x11"] }}
egui = "0.29"
zip = {{ version = "2", default-features = false, features = ["deflate"] }}
directories = "5"
image = {{ version = "0.25", default-features = false, features = ["png", "ico"] }}
rfd = "0.15"

[target.'cfg(target_os = "windows")'.dependencies]
winreg = "0.52"

{build_deps}
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "unwind"
"#
    );
    fs::write(build_root.join("Cargo.toml"), cargo_toml)?;

    // 4. build.rs — icon resource on Windows.
    let build_rs = if has_icon {
        r##"fn main() {
    println!("cargo:rerun-if-changed=icon.ico");
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icon.ico");
        if let Err(e) = res.compile() {
            eprintln!("warning: failed to embed icon: {e}");
        }
    }
}
"##
    } else {
        "fn main() {}\n"
    };
    fs::write(build_root.join("build.rs"), build_rs)?;

    // 5. The installer app itself.
    let main_rs = render_installer_main_rs(args, crate_name, has_icon);
    fs::write(build_root.join("src").join("main.rs"), main_rs)?;

    // 6. Compile and copy result.
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

/// Escape a string so it can be embedded as a Rust `&str` literal.
fn rs(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Render the full installer `main.rs`. The template is intentionally
/// literal (no templating engine): user-supplied strings are escaped and
/// interpolated only via `format!` `{}` placeholders.
fn render_installer_main_rs(args: &BuildArgs, crate_name: &str, has_icon: bool) -> String {
    let app_name = rs(&args.name);
    let identifier = rs(&args.identifier);
    let exe_name = rs(&format!("{crate_name}.exe"));
    let app_version = "0.1.0";
    let publisher = rs("Rizonet");

    // The window icon can be built from the same `icon.ico` file that
    // build.rs embeds. On systems without one, we fall back to egui's
    // default frame.
    let icon_loader = if has_icon {
        r#"
fn load_window_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../icon.ico");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width: w, height: h })
}
"#
    } else {
        r#"
fn load_window_icon() -> Option<egui::IconData> { None }
"#
    };

    format!(
        r##"#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

//! Auto-generated GUI installer for "{app_name_display}".
//!
//! Runs as a multi-page wizard (Welcome → Options → Installing → Finish)
//! or, when launched with `--uninstall`, as a matching uninstaller.

use std::fs;
use std::io::{{Read, Write}};
use std::path::{{Path, PathBuf}};
use std::process::Command;
use std::sync::{{Arc, Mutex}};
use std::thread;

use eframe::egui;

const APP_NAME: &str = "{app_name}";
const APP_VERSION: &str = "{app_version}";
const IDENTIFIER: &str = "{identifier}";
const EXE_NAME: &str = "{exe_name}";
const PUBLISHER: &str = "{publisher}";
const PAYLOAD: &[u8] = include_bytes!("../payload.zip");

{icon_loader}

fn default_install_dir() -> PathBuf {{
    directories::BaseDirs::new()
        .map(|b| b.data_local_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
        .join("Programs")
        .join(IDENTIFIER)
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

fn desktop_dir() -> Option<PathBuf> {{
    directories::UserDirs::new().and_then(|u| u.desktop_dir().map(Path::to_path_buf))
}}

fn create_shortcut(target_exe: &Path, shortcut_path: &Path) -> std::io::Result<()> {{
    if let Some(parent) = shortcut_path.parent() {{
        let _ = fs::create_dir_all(parent);
    }}
    let wd = target_exe.parent().unwrap_or_else(|| Path::new("."));
    let script = format!(
        "$s = (New-Object -COM WScript.Shell).CreateShortcut('{{}}'); \
         $s.TargetPath = '{{}}'; \
         $s.WorkingDirectory = '{{}}'; \
         $s.IconLocation = '{{}}'; \
         $s.Save();",
        shortcut_path.display(),
        target_exe.display(),
        wd.display(),
        target_exe.display(),
    );
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()?;
    Ok(())
}}

#[cfg(target_os = "windows")]
fn register_uninstall(install_dir: &Path, size_kb: u64) -> std::io::Result<()> {{
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = format!(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{{}}",
        IDENTIFIER
    );
    let (key, _) = hkcu.create_subkey(&path)?;
    let uninstaller = install_dir.join("uninstall.exe");
    let app_exe = install_dir.join(EXE_NAME);
    key.set_value("DisplayName", &APP_NAME.to_string())?;
    key.set_value("DisplayVersion", &APP_VERSION.to_string())?;
    key.set_value("Publisher", &PUBLISHER.to_string())?;
    key.set_value("InstallLocation", &install_dir.display().to_string())?;
    key.set_value("DisplayIcon", &app_exe.display().to_string())?;
    key.set_value(
        "UninstallString",
        &format!("\"{{}}\" --uninstall", uninstaller.display()),
    )?;
    key.set_value("NoModify", &1u32)?;
    key.set_value("NoRepair", &1u32)?;
    key.set_value("EstimatedSize", &(size_kb as u32))?;
    Ok(())
}}

#[cfg(not(target_os = "windows"))]
fn register_uninstall(_: &Path, _: u64) -> std::io::Result<()> {{ Ok(()) }}

#[cfg(target_os = "windows")]
fn unregister_uninstall() {{
    use winreg::enums::*;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = format!(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{{}}",
        IDENTIFIER
    );
    let _ = hkcu.delete_subkey_all(&path);
}}

#[cfg(not(target_os = "windows"))]
fn unregister_uninstall() {{}}

/// Extract the embedded payload zip into `dst`, reporting progress 0..=1.
fn extract_payload<F: FnMut(f32, &str)>(dst: &Path, mut on_progress: F) -> std::io::Result<u64> {{
    fs::create_dir_all(dst)?;
    let reader = std::io::Cursor::new(PAYLOAD);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let total = archive.len().max(1);
    let mut total_bytes: u64 = 0;
    for i in 0..archive.len() {{
        let mut entry = archive
            .by_index(i)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let name = entry.name().to_string();
        let out_path = dst.join(&name);
        if entry.is_dir() {{
            fs::create_dir_all(&out_path)?;
        }} else {{
            if let Some(p) = out_path.parent() {{
                fs::create_dir_all(p)?;
            }}
            let mut f = fs::File::create(&out_path)?;
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)?;
            f.write_all(&buf)?;
            total_bytes += buf.len() as u64;
        }}
        on_progress((i + 1) as f32 / total as f32, &name);
    }}
    Ok(total_bytes)
}}

/// Best-effort recursive removal.
fn remove_tree(p: &Path) {{
    if p.is_dir() {{
        let _ = fs::remove_dir_all(p);
    }} else if p.exists() {{
        let _ = fs::remove_file(p);
    }}
}}

/// ─────────────────────────────── GUI ──────────────────────────────

#[derive(PartialEq, Eq, Clone, Copy)]
enum Page {{ Welcome, Options, Installing, Finish, Error }}

struct Installer {{
    page: Page,
    uninstall_mode: bool,
    install_dir: String,
    start_menu: bool,
    desktop_shortcut: bool,
    launch_after: bool,
    progress: Arc<Mutex<(f32, String)>>,
    result: Arc<Mutex<Option<std::result::Result<PathBuf, String>>>>,
    job_started: bool,
    error_msg: String,
}}

impl Default for Installer {{
    fn default() -> Self {{
        Self {{
            page: Page::Welcome,
            uninstall_mode: false,
            install_dir: default_install_dir().display().to_string(),
            start_menu: true,
            desktop_shortcut: false,
            launch_after: true,
            progress: Arc::new(Mutex::new((0.0, String::new()))),
            result: Arc::new(Mutex::new(None)),
            job_started: false,
            error_msg: String::new(),
        }}
    }}
}}

impl Installer {{
    fn start_install(&mut self) {{
        self.job_started = true;
        let dir = PathBuf::from(self.install_dir.clone());
        let create_menu = self.start_menu;
        let create_desktop = self.desktop_shortcut;
        let progress = self.progress.clone();
        let result = self.result.clone();
        thread::spawn(move || {{
            let _ = fs::remove_dir_all(&dir);
            let total_bytes = match extract_payload(&dir, |p, name| {{
                *progress.lock().unwrap() = (p, name.to_string());
            }}) {{
                Ok(b) => b,
                Err(e) => {{
                    *result.lock().unwrap() = Some(Err(format!("extract failed: {{e}}")));
                    return;
                }}
            }};
            let exe = dir.join(EXE_NAME);

            // Copy the installer itself as the uninstaller.
            if let Ok(me) = std::env::current_exe() {{
                let _ = fs::copy(&me, dir.join("uninstall.exe"));
            }}

            if create_menu {{
                if let Some(menu) = start_menu_dir() {{
                    let lnk = menu.join(format!("{{}}.lnk", APP_NAME));
                    let _ = create_shortcut(&exe, &lnk);
                }}
            }}
            if create_desktop {{
                if let Some(d) = desktop_dir() {{
                    let lnk = d.join(format!("{{}}.lnk", APP_NAME));
                    let _ = create_shortcut(&exe, &lnk);
                }}
            }}
            let _ = register_uninstall(&dir, total_bytes / 1024);
            *result.lock().unwrap() = Some(Ok(exe));
        }});
    }}

    fn start_uninstall(&mut self) {{
        self.job_started = true;
        let dir = PathBuf::from(self.install_dir.clone());
        let progress = self.progress.clone();
        let result = self.result.clone();
        thread::spawn(move || {{
            *progress.lock().unwrap() = (0.3, "Removing shortcuts…".into());
            if let Some(menu) = start_menu_dir() {{
                remove_tree(&menu.join(format!("{{}}.lnk", APP_NAME)));
            }}
            if let Some(d) = desktop_dir() {{
                remove_tree(&d.join(format!("{{}}.lnk", APP_NAME)));
            }}
            *progress.lock().unwrap() = (0.6, "Removing registry entries…".into());
            unregister_uninstall();
            *progress.lock().unwrap() = (0.9, "Removing files…".into());
            // The running uninstaller may lock itself; schedule a cleanup .cmd.
            if dir.exists() {{
                #[cfg(target_os = "windows")]
                {{
                    let tmp = std::env::temp_dir().join(format!("rizonet_uninst_{{}}.cmd", IDENTIFIER));
                    let script = format!(
                        "@echo off\r\n\
                         timeout /t 1 >nul\r\n\
                         rmdir /S /Q \"{{}}\"\r\n\
                         del \"%~f0\"\r\n",
                        dir.display()
                    );
                    let _ = fs::write(&tmp, script);
                    let _ = Command::new("cmd").args(["/C", "start", "", "/MIN"]).arg(&tmp).spawn();
                }}
                #[cfg(not(target_os = "windows"))]
                {{
                    remove_tree(&dir);
                }}
            }}
            *progress.lock().unwrap() = (1.0, "Done".into());
            *result.lock().unwrap() = Some(Ok(dir));
        }});
    }}
}}

impl eframe::App for Installer {{
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {{
        ctx.request_repaint_after(std::time::Duration::from_millis(50));

        // Drive the state machine when a background job finishes.
        if matches!(self.page, Page::Installing) {{
            if let Some(res) = self.result.lock().unwrap().take() {{
                match res {{
                    Ok(_) => self.page = Page::Finish,
                    Err(e) => {{
                        self.error_msg = e;
                        self.page = Page::Error;
                    }}
                }}
            }}
        }}

        egui::TopBottomPanel::top("hdr").show(ctx, |ui| {{
            ui.add_space(8.0);
            ui.horizontal(|ui| {{
                ui.heading(if self.uninstall_mode {{
                    format!("Uninstall {{}}", APP_NAME)
                }} else {{
                    format!("{{}} Setup", APP_NAME)
                }});
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {{
                    ui.label(format!("v{{}}", APP_VERSION));
                }});
            }});
            ui.add_space(4.0);
            ui.separator();
        }});

        egui::TopBottomPanel::bottom("btns").show(ctx, |ui| {{
            ui.add_space(8.0);
            ui.horizontal(|ui| {{
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {{
                    match self.page {{
                        Page::Welcome => {{
                            if ui.button(if self.uninstall_mode {{ "Next ▶" }} else {{ "Next ▶" }}).clicked() {{
                                self.page = Page::Options;
                            }}
                            if ui.button("Cancel").clicked() {{
                                std::process::exit(0);
                            }}
                        }}
                        Page::Options => {{
                            let label = if self.uninstall_mode {{ "Uninstall" }} else {{ "Install" }};
                            if ui.button(label).clicked() {{
                                self.page = Page::Installing;
                                if self.uninstall_mode {{
                                    self.start_uninstall();
                                }} else {{
                                    self.start_install();
                                }}
                            }}
                            if ui.button("◀ Back").clicked() {{
                                self.page = Page::Welcome;
                            }}
                        }}
                        Page::Installing => {{
                            ui.add_enabled(false, egui::Button::new("Please wait…"));
                        }}
                        Page::Finish | Page::Error => {{
                            if ui.button("Close").clicked() {{
                                if matches!(self.page, Page::Finish)
                                    && !self.uninstall_mode
                                    && self.launch_after
                                {{
                                    let exe = PathBuf::from(&self.install_dir).join(EXE_NAME);
                                    let _ = Command::new(&exe).spawn();
                                }}
                                std::process::exit(0);
                            }}
                        }}
                    }}
                }});
            }});
            ui.add_space(6.0);
        }});

        egui::CentralPanel::default().show(ctx, |ui| {{
            ui.add_space(10.0);
            match self.page {{
                Page::Welcome => {{
                    ui.heading(if self.uninstall_mode {{
                        format!("Remove {{}} from this computer", APP_NAME)
                    }} else {{
                        format!("Welcome to the {{}} Setup", APP_NAME)
                    }});
                    ui.add_space(10.0);
                    if self.uninstall_mode {{
                        ui.label(format!(
                            "This wizard will remove {{}} (v{{}}), its shortcuts and its registry entries.",
                            APP_NAME, APP_VERSION
                        ));
                    }} else {{
                        ui.label(format!(
                            "This wizard will install {{}} (v{{}}) on your computer.",
                            APP_NAME, APP_VERSION
                        ));
                        ui.add_space(6.0);
                        ui.label("Click Next to continue, or Cancel to exit.");
                    }}
                }}
                Page::Options => {{
                    if self.uninstall_mode {{
                        ui.heading("Ready to uninstall");
                        ui.add_space(10.0);
                        ui.label(format!("Location:\n  {{}}", self.install_dir));
                        ui.add_space(6.0);
                        ui.label("Click Uninstall to proceed.");
                    }} else {{
                        ui.heading("Choose install options");
                        ui.add_space(8.0);
                        ui.label("Install location");
                        ui.horizontal(|ui| {{
                            ui.add(
                                egui::TextEdit::singleline(&mut self.install_dir)
                                    .desired_width(ui.available_width() - 90.0),
                            );
                            if ui.button("Browse…").clicked() {{
                                if let Some(d) = rfd::FileDialog::new()
                                    .set_directory(&self.install_dir)
                                    .pick_folder()
                                {{
                                    self.install_dir = d.display().to_string();
                                }}
                            }}
                        }});
                        ui.add_space(12.0);
                        ui.checkbox(&mut self.start_menu, "Create a Start Menu shortcut");
                        ui.checkbox(&mut self.desktop_shortcut, "Create a Desktop shortcut");
                        ui.checkbox(&mut self.launch_after, format!("Launch {{}} after installing", APP_NAME));
                    }}
                }}
                Page::Installing => {{
                    ui.heading(if self.uninstall_mode {{ "Uninstalling…" }} else {{ "Installing…" }});
                    ui.add_space(10.0);
                    let (p, msg) = self.progress.lock().unwrap().clone();
                    ui.add(egui::ProgressBar::new(p).show_percentage());
                    ui.add_space(6.0);
                    ui.label(msg);
                    if !self.job_started {{
                        // Safety net — shouldn't happen, but guarantees the job kicks off.
                        if self.uninstall_mode {{ self.start_uninstall() }} else {{ self.start_install() }};
                    }}
                }}
                Page::Finish => {{
                    ui.heading(if self.uninstall_mode {{
                        format!("{{}} has been removed", APP_NAME)
                    }} else {{
                        format!("{{}} installed successfully", APP_NAME)
                    }});
                    ui.add_space(10.0);
                    if !self.uninstall_mode {{
                        ui.label(format!("Installed to:\n  {{}}", self.install_dir));
                        ui.add_space(6.0);
                        ui.checkbox(&mut self.launch_after, format!("Launch {{}} when I close this window", APP_NAME));
                    }}
                }}
                Page::Error => {{
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), "Something went wrong");
                    ui.add_space(10.0);
                    ui.label(&self.error_msg);
                }}
            }}
        }});
    }}
}}

fn main() -> std::result::Result<(), eframe::Error> {{
    let uninstall_mode = std::env::args().any(|a| a == "--uninstall");
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([560.0, 380.0])
        .with_min_inner_size([520.0, 340.0])
        .with_title(if uninstall_mode {{
            format!("Uninstall {{}}", APP_NAME)
        }} else {{
            format!("{{}} Setup", APP_NAME)
        }});
    if let Some(icon) = load_window_icon() {{
        viewport = viewport.with_icon(Arc::new(icon));
    }}
    let options = eframe::NativeOptions {{
        viewport,
        ..Default::default()
    }};
    eframe::run_native(
        &format!("{{}} Setup", APP_NAME),
        options,
        Box::new(move |_cc| {{
            let mut app = Installer::default();
            app.uninstall_mode = uninstall_mode;
            Ok(Box::new(app))
        }}),
    )
}}
"##,
        app_name_display = args.name.replace('"', "\\\""),
        app_name = app_name,
        app_version = app_version,
        identifier = identifier,
        exe_name = exe_name,
        publisher = publisher,
        icon_loader = icon_loader,
    )
}
