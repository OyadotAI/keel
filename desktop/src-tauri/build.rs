// Build the desktop and its small native WK ownership adapter without a package dependency.
fn main() {
    build_browser_debug();
    // Every app command named, so each one is granted per webview by a capability rather than
    // reachable from any page with IPC. The preview webview loads the dev server — the project's
    // dependencies' code — and capabilities/preview.json gives it `preview_msg` and nothing else.
    const COMMANDS: &[&str] = &[
        "open_project",
        "close_project",
        "stop_all",
        "scratch_dir",
        "projects_dir",
        "in_workspace",
        "sign_packet",
        "save_text",
        "is_dir",
        "read_dropped",
        "preview_show",
        "preview_bounds",
        "preview_hide",
        "preview_send",
        "preview_msg",
        "browser_act",
        "browser_place",
        "browser_close",
    ];
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("the Tauri build");
}

#[cfg(target_os = "macos")]
fn build_browser_debug() {
    use std::{env, path::PathBuf, process::Command};
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output"));
    let source = "src/browser/debug/native.mm";
    println!("cargo:rerun-if-changed={source}");
    let object = output.join("keel-browser-debug.o");
    let target = env::var("CARGO_CFG_TARGET_ARCH").expect("target architecture");
    let arch = if target == "aarch64" {
        "arm64"
    } else {
        "x86_64"
    };
    assert!(
        Command::new("xcrun")
            .args([
                "clang++",
                "-arch",
                arch,
                "-std=c++17",
                "-fobjc-arc",
                "-mmacosx-version-min=14",
                "-c",
                source,
                "-o"
            ])
            .arg(&object)
            .status()
            .expect("compile WK bridge")
            .success()
    );
    assert!(
        Command::new("ar")
            .arg("rcs")
            .arg(output.join("libkeel_browser_debug.a"))
            .arg(object)
            .status()
            .expect("archive WK bridge")
            .success()
    );
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=keel_browser_debug");
    // Cargo's example target needs its own archive arguments; it exercises the real desktop adapter.
    println!(
        "cargo:rustc-link-arg-examples={}",
        output.join("libkeel_browser_debug.a").display()
    );
    for argument in ["-framework", "JavaScriptCore", "-lc++"] {
        println!("cargo:rustc-link-arg-examples={argument}");
    }

    for framework in ["WebKit", "JavaScriptCore", "AppKit"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    println!("cargo:rustc-link-lib=c++");
}
#[cfg(not(target_os = "macos"))]
fn build_browser_debug() {}
