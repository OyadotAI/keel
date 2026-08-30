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

    var available: Bool { controller != nil }

    func start() {
        let info = Bundle.main.infoDictionary ?? [:]
        guard let key = info["SUPublicEDKey"] as? String, !key.isEmpty,
              let feed = info["SUFeedURL"] as? String, !feed.isEmpty else { return }
        // Not during launch. Starting the updater reads the keychain and schedules a check,
        // and doing it while the first window is laying out put Sparkle in the app's own
        // launch hang. Ten seconds later nobody is waiting for it.
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(10))
            let c = SPUStandardUpdaterController(startingUpdater: true,
                                                 updaterDelegate: nil, userDriverDelegate: nil)
            // Offered, never silent: an update that downloads and installs itself under
            // somebody mid-turn is the App Hang in Sentry, and a surprise either way.
            c.updater.automaticallyDownloadsUpdates = false
            c.updater.automaticallyChecksForUpdates = true
            controller = c
        }
    }

    func check() {
        controller?.checkForUpdates(nil)
    }
}
