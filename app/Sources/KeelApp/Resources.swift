import Foundation

/// Files that ship with the app, found without trapping.
///
/// SwiftPM's `Bundle.module` looks for `KeelApp_KeelApp.bundle` beside the executable or in the
/// build directory, and calls `fatalError` when neither exists. The packaging script had never
/// copied that bundle into `Keel.app`; on the machine that built it the build directory was
/// there, so it worked, and on every other machine the Designer crashed on open. Resources are
/// now copied to `Contents/Resources` by `build-app.sh` and looked up here — the packaged
/// location first, then the development bundle, and a missing file is `nil`, never a crash.
enum Resources {
    static func url(_ name: String, _ ext: String) -> URL? {
        if let u = Bundle.main.url(forResource: name, withExtension: ext) { return u }
        // Development: `swift run` / `swift test`, where SwiftPM's bundle exists on disk.
        let dev = Bundle.main.bundleURL.appendingPathComponent("KeelApp_KeelApp.bundle")
        if let b = Bundle(url: dev), let u = b.url(forResource: name, withExtension: ext) { return u }
        if let b = Bundle.allBundles.first(where: { $0.bundlePath.hasSuffix("KeelApp_KeelApp.bundle") }),
           let u = b.url(forResource: name, withExtension: ext) { return u }
        #if DEBUG
        // Last resort in debug builds only, where the build directory is guaranteed to exist.
        return Bundle.module.url(forResource: name, withExtension: ext)
        #else
        return nil
        #endif
    }

    static func text(_ name: String, _ ext: String) -> String? {
        url(name, ext).flatMap { try? String(contentsOf: $0, encoding: .utf8) }
    }
}
