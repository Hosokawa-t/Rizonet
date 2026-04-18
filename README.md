# ⚡ Rizonet

> **As light and fast as Tauri, as environment-independent as Electron.**
> A desktop-app framework that aims for the best of both worlds.

---

## 🎯 Why Rizonet

| Trait | Tauri | Electron | **Rizonet** |
|---|---|---|---|
| Binary size | ~3 MB | ~150 MB | **~5 MB + on-demand runtime** |
| Startup speed | Fast | Slow | **Fast** |
| Environment-dependent | Yes (system WebView) | No (bundles Chromium) | **No (auto-bootstrap)** |
| Rendering consistency | Depends on OS | Fully consistent | **Fully consistent in `strict` mode** |
| Backend language | Rust | Node.js | **Rust** |

## 🧠 Architecture: *Hybrid Runtime Bootstrap*

```
┌──────────────────────────── Rizonet App ────────────────────────────┐
│                                                                     │
│   JS / HTML / CSS   ←―  window.__RIZONET__.invoke(cmd, payload)     │
│          ↑                                                          │
│   ┌──────┴──────┐                                                   │
│   │  WebView    │  ← wry / tao (OS-native) OR                       │
│   │  (chosen)   │    bundled runtime (downloaded, cached, verified) │
│   └──────┬──────┘                                                   │
│          │ IPC (JSON)                                               │
│   ┌──────┴──────┐                                                   │
│   │  rizonet-   │  Rust backend                                     │
│   │  core       │  (command handlers / FS / net / plugins / …)      │
│   └─────────────┘                                                   │
└─────────────────────────────────────────────────────────────────────┘
```

At startup, the WebView is selected according to `runtime.mode`:

- **`auto`** (default) — Use the OS-provided WebView. Fall back to a
  downloaded, pinned runtime if it's missing or older than
  `runtime.min_webview_version`.
- **`system`** — Always use the OS WebView (Tauri-style, lightest).
- **`strict`** — Always use the bundled runtime (Electron-style rendering
  consistency).

The bundled runtime is stored in the OS-standard cache directory with a
SHA-256 integrity check, so subsequent launches are instant.

On Windows, the bundled mode wires WebView2 through the official
`WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` contract, so you can ship a
fixed-version WebView2 Runtime ZIP and get pixel-identical rendering
across every Windows install.

## 📂 Repository layout

```
crates/
  rizonet-core/       # Rust core (webview + runtime + IPC + plugins + updater)
  rizonet-cli/        # `rizonet` command (new / dev / build / info / update-check)
packages/
  rizonet-api/        # TypeScript bindings
examples/
  hello/              # Working sample
```

## 🚀 Quick start

### 1. Build the CLI

```powershell
cargo build -p rizonet-cli --release
# Binary: target/release/rizonet(.exe)
```

### 2. Scaffold a project

```powershell
./target/release/rizonet new my-app
cd my-app
rizonet dev
```

### 3. Or just run the sample

```powershell
cargo run -p hello-rizonet
```

A window opens and the buttons exercise the IPC bridge:
`ping`, `app_version`, `platform_key`, `clock:now` (from a plugin).

### 4. Or package a web app **with a GUI** — Rizonet Studio

```powershell
cargo run -p rizonet-studio --release
```

Rizonet Studio is itself a Rizonet app. Pick any web folder containing
`index.html`, an output folder, fill in the name/identifier, hit **Build
native app**, and watch the live `cargo build --release` log. The
resulting `<name>.exe`, `dist/`, and `rizonet.config.json` are dropped
into the output folder, ready to ship.

Headless end-to-end test (packages `examples/hello`):

```powershell
cargo test -p rizonet-studio --test e2e_build --release -- --ignored --nocapture
```

## 🔌 IPC

### Rust

```rust
use rizonet_core::{AppBuilder, IpcHandler, RizonetConfig};
use serde_json::json;

let ipc = IpcHandler::new()
    .register("ping", |payload| Ok(json!({ "pong": true, "echo": payload })));

AppBuilder::new(cfg).ipc(ipc).run()?;
```

### TypeScript / JavaScript

```ts
import { invoke } from "@rizonet/api";
const r = await invoke<{ pong: boolean }>("ping", { hello: "world" });
```

Or raw DOM:

```js
const r = await window.__RIZONET__.invoke("ping", { hello: "world" });
```

## 🧩 Plugins

```rust
use rizonet_core::{IpcHandler, Plugin, Result};
use serde_json::json;

struct Clock;
impl Plugin for Clock {
    fn name(&self) -> &str { "clock" }
    fn register(&self, ipc: IpcHandler) -> IpcHandler {
        ipc.register("clock:now", |_| Ok(json!({ "ts": 0 })))
    }
    fn on_start(&self) -> Result<()> { Ok(()) }
}

AppBuilder::new(cfg).plugin(Clock).run()?;
```

## 🔄 Auto-updater

Host a JSON manifest:

```json
{
  "version": "0.2.0",
  "notes": "What's new",
  "artifacts": {
    "windows-x86_64": { "url": "https://.../my-app.exe", "sha256": "..." },
    "linux-x86_64":   { "url": "https://.../my-app",     "sha256": "..." },
    "macos-aarch64":  { "url": "https://.../my-app.dmg", "sha256": "..." }
  }
}
```

Check from Rust:

```rust
use rizonet_core::updater;

if let Some(info) = updater::check("https://example.com/update.json", env!("CARGO_PKG_VERSION")).await? {
    let staged = updater::download_and_stage(&info).await?;
    println!("Update staged at {:?}", staged);
}
```

Or from the CLI:

```powershell
rizonet update-check --manifest https://example.com/update.json --current 0.1.0
```

The staged artifact is placed next to the current executable with a
`.rizonet-update` suffix, after SHA-256 verification. The host app is
responsible for swapping it in and restarting (platform-specific).

## ⚙️ `rizonet.config.json`

```jsonc
{
  "app": {
    "name": "My App",
    "identifier": "net.rizonet.myapp",
    "dist_dir": "dist"
  },
  "window": { "title": "My App", "width": 1024, "height": 768 },
  "runtime": {
    "mode": "auto",                     // "auto" | "system" | "strict"
    "min_webview_version": "110.0.0",   // optional
    "bootstrap_url": "https://.../runtime.zip",
    "bootstrap_sha256": "..."
  }
}
```

## 🧰 Built-in IPC commands

Every Rizonet app gets these commands for free (gated by the `capabilities`
allow/deny policy):

| Namespace | Commands |
|---|---|
| `dialog:*` | `open_file`, `save_file`, `message`, `confirm` |
| `notification:*` | `send` |
| `clipboard:*` | `read_text`, `write_text` |
| `shell:*` | `open` (URL or path) |
| `path:*` | `home`, `config`, `data`, `cache`, `documents`, `desktop`, `download`, `temp` |
| `http:*` | `fetch` (CORS-free, Rust-driven HTTP) |
| `window:*` | `show`, `hide`, `minimize`, `unminimize`, `maximize`, `unmaximize`, `focus`, `set_title`, `close` |
| `devtools:*` | `open`, `close` |
| `event:*` | `emit` (JS re-broadcasts an event through Rust) |

### Rust → JS events

```rust
rizonet_core::handle().emit("notes:saved", json!({ "name": "foo.md" }))?;
```

```js
window.__RIZONET__.on("notes:saved", (p) => console.log("saved:", p));
```

### Capabilities policy

```jsonc
{
  "capabilities": {
    "allow": ["notes:*", "dialog:*", "clipboard:*", "window:*"],
    "deny":  ["dialog:save_file"]
  }
}
```

Patterns support a single `*` wildcard. An empty `allow` list means "allow all".

### Managed state

```rust
struct Db(Mutex<Vec<String>>);
AppBuilder::new(cfg).manage(Db(Mutex::new(vec![]))).run()?;

// From any IPC handler:
let db = rizonet_core::handle().state::<Db>().unwrap();
db.0.lock().unwrap().push("hi".into());
```

## 🛠️ Implementation status

- [x] Cargo workspace, CLI (`new` / `dev` / `build` / `info` / `update-check`)
- [x] Window + WebView via `wry` + `tao`
- [x] JS ⇄ Rust IPC with promise resolution via `evaluate_script`
- [x] Rust → JS events (`emit` / `on` / `off` / `once`)
- [x] Runtime resolution (`auto` / `system` / `strict`)
- [x] SHA-256 verified download + ZIP extraction + cache
- [x] Windows WebView2 version detection and fixed-version wiring
- [x] Linux WebKitGTK presence detection
- [x] Plugin API (trait-based, lifecycle hooks)
- [x] Auto-updater (manifest fetch, verified DL, staging)
- [x] Built-in commands: dialog / notification / clipboard / shell / path / HTTP
- [x] Window control + devtools toggle via `AppHandle`
- [x] Type-indexed managed state (`manage` / `handle().state::<T>()`)
- [x] Capability-based IPC ACL (`allow` / `deny` with globs)
- [x] Cross-thread `AppHandle` via Tao `EventLoopProxy`
- [ ] System tray & native menus (`tray-icon` / `muda` integration)
- [ ] Global shortcuts (`global-hotkey` integration)
- [ ] Installer bundling (MSI / NSIS / DMG / DEB / AppImage)
- [ ] Apply staged update + restart (platform-specific)
- [ ] Code signing helpers

## 📄 License

MIT OR Apache-2.0
