//! The macOS application: a real window with a WKWebView in it.
//!
//! Keel's UI is served over loopback, so it would be easy to call a browser tab the application
//! and stop. It is not the same thing. A tab has no menu bar, so ⌘Q does nothing, ⌘C in a text
//! field does nothing, and there is no Window menu; it has no Dock icon of its own, so ⌘Tab skips
//! it; it inherits whatever zoom, extensions and theme the browser has; and it closes when someone
//! tidies their tabs. This module is what makes it an application instead.
//!
//! # Why the event loop owns `main`
//!
//! AppKit requires its run loop on the process's first thread and will abort if it is anywhere
//! else. So the server moves: a Tokio runtime is started on a worker thread, the window is built
//! on the main thread, and the two only meet over the loopback port. That is also why the window
//! waits for the server to answer before it loads — a WKWebView pointed at a socket that is not
//! listening yet shows its own connection-failure page and does not retry.

use anyhow::{Context, Result};
use muda::{AboutMetadata, Menu, PredefinedMenuItem, Submenu};
use tao::{
    dpi::LogicalSize,
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

/// Run Keel as an application.
pub fn run(port: u16) -> Result<()> {
    // The server first, on its own thread. `run_app` reopens the last project, or serves the
    // welcome screen when there is none.
    std::thread::Builder::new()
        .name("keel-server".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("starting the async runtime");
            if let Err(e) = runtime.block_on(crate::serve::run_app(port, false)) {
                eprintln!("keel: server stopped: {e:#}");
            }
        })
        .context("starting the server thread")?;

    let url = format!("http://127.0.0.1:{port}");
    wait_for_server(port);

    let event_loop = EventLoopBuilder::new().build();

    let (w, h) = crate::prefs::Prefs::load()
        .window
        // Wide enough for the sidebar, the editor and the agent panel at once, which is the
        // layout the whole UI is designed around. Narrower than this and one of them has to go.
        .unwrap_or((1440.0, 900.0));

    let window = WindowBuilder::new()
        .with_title("Keel")
        .with_inner_size(LogicalSize::new(w, h))
        .with_min_inner_size(LogicalSize::new(880.0, 560.0))
        .build(&event_loop)
        .context("creating the window")?;

    menu_bar()?;

    let _webview = WebViewBuilder::new()
        .with_url(&url)
        // The UI is Keel's own, served from loopback, and the developer tools are how anyone
        // reports a rendering bug in it with something more useful than a screenshot.
        .with_devtools(true)
        .with_back_forward_navigation_gestures(false)
        .build(&window)
        .context("creating the webview")?;

    let mut size = (w, h);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {}
            Event::WindowEvent {
                event: WindowEvent::Resized(new),
                ..
            } => {
                let logical = new.to_logical::<f64>(window.scale_factor());
                size = (logical.width, logical.height);
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            // Reached by closing the window and by ⌘Q alike, so the size is saved either way.
            Event::LoopDestroyed => crate::prefs::Prefs::remember_window(size.0, size.1),
            _ => {}
        }
    });
}

/// Give the server a moment to bind before the webview asks for a page.
///
/// A WKWebView that gets a refused connection renders its own error page and stays there — there
/// is no retry and no reload. Polling the port for a second is the whole fix.
fn wait_for_server(port: u16) {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    for _ in 0..100 {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The menu bar.
///
/// Not decoration: on macOS the Edit menu is what makes ⌘X, ⌘C, ⌘V and ⌘A work at all. A WKWebView
/// with no Edit menu is a window where you cannot copy out of a text field, which is the single
/// most common way a webview-in-a-bundle gives itself away.
///
/// Everything else Keel does has its own shortcut inside the page, and duplicating those here
/// would mean two definitions of every command that can disagree. So the menu bar carries what
/// only the system can provide, and nothing more.
fn menu_bar() -> Result<()> {
    let menu = Menu::new();

    let about = AboutMetadata {
        name: Some("Keel".into()),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        comments: Some("The local IDE that makes a repository shippable.".into()),
        ..Default::default()
    };

    menu.append(&Submenu::with_items(
        "Keel",
        true,
        &[
            &PredefinedMenuItem::about(None, Some(about)),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(None),
        ],
    )?)?;

    menu.append(&Submenu::with_items(
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(None),
            &PredefinedMenuItem::redo(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::cut(None),
            &PredefinedMenuItem::copy(None),
            &PredefinedMenuItem::paste(None),
            &PredefinedMenuItem::select_all(None),
        ],
    )?)?;

    menu.append(&Submenu::with_items(
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(None),
            &PredefinedMenuItem::maximize(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::close_window(None),
            &PredefinedMenuItem::fullscreen(None),
        ],
    )?)?;

    // Installing the menu is the one genuinely platform-bound step: `init_for_nsapp` hands the
    // menu to AppKit as the process-wide bar, and it exists only in muda's macOS build. Keel ships
    // on macOS, so there is nothing to install anywhere else, but CI compiles on Linux for cheap
    // minutes and the rest of this module is portable enough to be worth type-checking there.
    #[cfg(target_os = "macos")]
    menu.init_for_nsapp();
    Ok(())
}
