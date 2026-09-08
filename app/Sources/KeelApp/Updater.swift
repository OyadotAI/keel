import Foundation
import Sparkle
import SwiftUI

/// Auto-update, so builds stop travelling by hand.
///
/// Sparkle, because it is what every macOS app outside the App Store uses and the reasons are
/// boring: EdDSA-signed appcasts, delta updates, an installer that survives the app quitting.
/// The feed is a file on the GitHub release — `releases/latest/download/appcast.xml` — so
/// nothing is hosted. The updater only starts when the bundle carries a public key; a
/// developer's `swift run` has none and gets no update prompts.
@MainActor
final class Updater {
    static let shared = Updater()
    private var controller: SPUStandardUpdaterController?

    /// Whether this build can update at all — a fact about the bundle, settled before the first
    /// window draws and true for the rest of the process.
    ///
    /// It used to be `controller != nil`, which is a *race*: the controller is deliberately built
    /// ten seconds after launch, and this class is not observable, so the menu item read `false`
    /// while the menu was being constructed and SwiftUI had nothing to tell it otherwise. Check
    /// for Updates was greyed out for the life of the app, on every version.
    var available: Bool { Self.configured != nil }

    private static var configured: (key: String, feed: String)? {
        let info = Bundle.main.infoDictionary ?? [:]
        guard let key = info["SUPublicEDKey"] as? String, !key.isEmpty,
              let feed = info["SUFeedURL"] as? String, !feed.isEmpty else { return nil }
        return (key, feed)
    }

    func start() {
        guard Self.configured != nil else { return }
        // Not during launch. Starting the updater reads the keychain and schedules a check,
        // and doing it while the first window is laying out put Sparkle in the app's own
        // launch hang. Ten seconds later nobody is waiting for it.
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(10))
            if controller == nil { controller = makeController() }
        }
    }

    /// Check now, whoever asks first.
    ///
    /// If the deferred start has not run yet, the controller is built here rather than the menu
    /// doing nothing — pressing Check for Updates in the first ten seconds is a reasonable thing
    /// to do, and silence is the worst answer to it.
    func check() {
        if controller == nil { controller = makeController() }
        controller?.checkForUpdates(nil)
    }

    private func makeController() -> SPUStandardUpdaterController? {
        guard Self.configured != nil else { return nil }
        let c = SPUStandardUpdaterController(startingUpdater: true,
                                             updaterDelegate: nil, userDriverDelegate: nil)
        // Offered, never silent: an update that downloads and installs itself under
        // somebody mid-turn is the App Hang in Sentry, and a surprise either way.
        c.updater.automaticallyDownloadsUpdates = false
        c.updater.automaticallyChecksForUpdates = true
        return c
    }
}
