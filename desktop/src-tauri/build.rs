fn main() {
    // Every app command named, so each one is granted per webview by a capability rather than
    // reachable from any page with IPC. The preview webview loads the dev server — the project's
    // dependencies' code — and capabilities/preview.json gives it `preview_msg` and nothing else.
    const COMMANDS: &[&str] = &[
        "open_project",
        "close_project",
        "stop_all",
        "scratch_dir",
        "sign_packet",
        "save_text",
        "is_dir",
        "preview_show",
        "preview_bounds",
        "preview_hide",
        "preview_send",
        "preview_msg",
    ];
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)))
        .expect("the Tauri build");
}
