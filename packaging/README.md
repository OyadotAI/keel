# Packaging

`make app` builds `dist/Keel.app`; `make dmg` wraps it in `dist/Keel.dmg` with a drag-to-Applications
alias. Both need only a Mac and a release build — no Xcode project, no signing certificate.

## What the bundle is

A real macOS application: an `NSWindow` with a `WKWebView` in it, a Dock icon, a menu bar and ⌘Q.
The UI is served over loopback by the same axum server the CLI runs, and the window loads it — but
the window is the application, not a browser wearing one.

That distinction is worth stating because the shortcut is tempting. A browser tab opened with
`--app=` looks close: no tab strip, no address bar. It has no menu bar, so ⌘Q does nothing and ⌘C
in a text field does nothing; no Dock icon of its own, so ⌘Tab skips it; whatever zoom, theme and
extensions the browser carries; and it disappears when somebody tidies their tabs. It was the
first version of this and it was not an application.

Three things follow from the architecture:

- **AppKit owns the main thread.** Its run loop has to be the process's first thread or it aborts,
  so the Tokio runtime is what moves to a worker thread. The two meet only over the loopback port.
- **The window waits for the port.** A `WKWebView` given a refused connection renders its own error
  page and never retries, so the port is polled before the first load.
- **The Edit menu is load-bearing.** On macOS ⌘X/⌘C/⌘V/⌘A in a webview are provided by the menu
  bar, not by the webview. Ship without one and text fields silently stop accepting paste — the
  most common tell of a webview in a bundle.

Launching twice raises what is already open rather than colliding on the port. The window's size is
remembered between launches.

## Signing

The bundle is signed ad-hoc (`codesign -s -`), which is enough to run on the machine that built it.
Anyone else's Mac will refuse it on first open — Gatekeeper wants a Developer ID and a notarisation
ticket, which need a paid Apple Developer account. Until there is one:

    xattr -dr com.apple.quarantine /Applications/Keel.app

is what a user has to run, and saying so plainly is better than shipping something that fails with
"Keel is damaged and can't be opened", which is what Gatekeeper says instead of the truth.

To sign properly once an account exists:

    codesign --deep --force --options runtime --sign "Developer ID Application: …" dist/Keel.app
    xcrun notarytool submit dist/Keel.dmg --keychain-profile … --wait
    xcrun stapler staple dist/Keel.dmg
