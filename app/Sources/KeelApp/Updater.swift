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
        controller = SPUStandardUpdaterController(startingUpdater: true,
                                                  updaterDelegate: nil, userDriverDelegate: nil)
    }

    func check() {
        controller?.checkForUpdates(nil)
    }
}
