//! End-to-end test: drive the packaging pipeline the way the GUI does.
//!
//! This test runs a real `cargo build --release` for the generated
//! project and therefore takes minutes on a cold cache. Enable with:
//!
//! ```text
//! cargo test -p rizonet-studio --test e2e_build -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use rizonet_studio::builder::{self, BuildArgs};

#[test]
#[ignore]
fn packages_hello_example_into_a_standalone_binary() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .expect("workspace root");
    let source = workspace.join("examples").join("hello").join("dist");
    assert!(source.join("index.html").is_file(), "expected hello/dist");

    let tmp = tempfile::tempdir().expect("tempdir");
    let output = tmp.path().to_path_buf();

    let args = BuildArgs {
        source_dir: source.display().to_string(),
        output_dir: output.display().to_string(),
        name: "Hello Bundled".into(),
        identifier: "net.rizonet.hello_bundled".into(),
        title: "Hello Bundled".into(),
        mode: "auto".into(),
        window_mode: "windowed".into(),
        width: 800.0,
        height: 600.0,
        resizable: true,
        allow: vec!["event:emit".into()],
        make_installer: false,
    };

    let logger: builder::Logger = Box::new(|stream, line| {
        eprintln!("[{stream}] {line}");
    });

    let outcome = builder::run(args, logger).expect("build succeeds");

    let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
    let expected_exe = output.join(format!("hello_bundled{exe_suffix}"));
    assert_eq!(outcome.binary, expected_exe);
    assert!(outcome.binary.is_file(), "exe not written");
    assert!(outcome.dist.join("index.html").is_file(), "dist not copied");
    assert!(outcome.config.is_file(), "config not written");

    // Basic config sanity.
    let cfg_text = std::fs::read_to_string(&outcome.config).expect("read config");
    assert!(cfg_text.contains("Hello Bundled"));
    assert!(cfg_text.contains("net.rizonet.hello_bundled"));
}
