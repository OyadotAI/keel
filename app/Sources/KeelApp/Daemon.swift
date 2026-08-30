import Foundation

/// The `keel serve` process this app drives.
///
/// A child process rather than a linked library. The Rust side is where the scanner, the workspace
/// reader and the whole permission model live, and it already speaks HTTP to a UI — keeping that
/// boundary means the daemon stays independently runnable (`keel serve`, `keel scan`) and this app
/// stays a client rather than a fork of it.
///
/// One daemon, and it dies with the app. Orca's worst reported bug is exactly the other thing:
/// "app updates leave previous daemon generations running forever — invisible agent sessions
/// accumulate and exhaust memory". A child process in a terminated group cannot outlive its parent
/// by accident, and the test in `KeelAppTests` counts them.
@MainActor
final class Daemon {
    let port: UInt16
    private var process: Process?

    init(port: UInt16 = 7777) { self.port = port }

    /// The `keel` binary: beside this executable inside the bundle, or on PATH when running from
    /// a checkout.
    static func binary() -> URL? {
        let beside = Bundle.main.bundleURL
            .appendingPathComponent("Contents/MacOS/keel")
        if FileManager.default.isExecutableFile(atPath: beside.path) { return beside }

        let sibling = URL(fileURLWithPath: CommandLine.arguments[0])
            .deletingLastPathComponent()
            .appendingPathComponent("keel")
        if FileManager.default.isExecutableFile(atPath: sibling.path) { return sibling }

        for dir in (ProcessInfo.processInfo.environment["PATH"] ?? "").split(separator: ":") {
            let candidate = URL(fileURLWithPath: String(dir)).appendingPathComponent("keel")
            if FileManager.default.isExecutableFile(atPath: candidate.path) { return candidate }
        }
        return nil
    }

    /// Start it, unless one is already answering on this port.
    ///
    /// `keel serve` already refuses to double-bind and prints "Keel is already running", so a
    /// stale daemon from a crashed run is joined rather than fought with.
    /// True when something already answers on this port.
    func answering() async -> Bool { await isUp() }

    func start(resumeLast: Bool = true) async throws {
        if await isUp() { return }
        guard let bin = Self.binary() else {
            throw Failure("Could not find the `keel` binary next to this app or on PATH.")
        }
        let p = Process()
        p.executableURL = bin
        // `--exit-with-parent` is the part that actually holds. `applicationWillTerminate` runs
        // on a ⌘Q and on nothing else: SIGTERM, a force quit and a crash all skip it, and macOS
        // has no `PR_SET_PDEATHSIG` for the child to notice with. So the daemon watches instead.
        // `--resume-last` because a Finder launch has no working directory worth inferring a
        // project from: it is `/`, and the default would open the whole filesystem as a repo.
        p.arguments = ["serve", "--port", String(port), "--no-open", "--exit-with-parent"]
        // Only the first window infers a project from the last one; a second window is told
        // which repository it is for.
        if resumeLast { p.arguments! += ["--resume-last"] }
        // The daemon reports crashes under the app's key, tagged as the daemon.
        if let dsn = Bundle.main.infoDictionary?["KeelSentryDSN"] as? String, !dsn.isEmpty {
            p.arguments! += ["--sentry-dsn", dsn]
        }
        try p.run()
        process = p

        // Poll rather than sleep: the daemon reads the repository on the way up and how long that
        // takes is a property of the repository, not a constant anyone can pick here.
        for _ in 0..<100 {
            if await isUp() { return }
            try? await Task.sleep(for: .milliseconds(50))
        }
        throw Failure("The Keel daemon did not answer on port \(port).")
    }

    func stop() {
        process?.terminate()
        process = nil
    }

    private func isUp() async -> Bool {
        var req = URLRequest(url: URL(string: "http://127.0.0.1:\(port)/api/state")!)
        req.timeoutInterval = 1
        guard let (_, response) = try? await URLSession.shared.data(for: req),
              let http = response as? HTTPURLResponse else { return false }
        return http.statusCode == 200
    }

    struct Failure: Error, LocalizedError {
        let message: String
        init(_ m: String) { message = m }
        var errorDescription: String? { message }
    }
}
