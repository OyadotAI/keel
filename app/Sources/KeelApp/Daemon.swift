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
    private let session: URLSession
    private var generation = 0

    init(port: UInt16 = 7777, session: URLSession = .shared) {
        self.port = port
        self.session = session
    }

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

    static func launchArguments(port: UInt16, resumeLast: Bool, sentryDSN _: String?) -> [String] {
        var arguments = ["serve", "--port", String(port), "--exit-with-parent"]
        if resumeLast { arguments += ["--resume-last"] }
        // A separate daemon Sentry client cannot observe the app's live Privacy toggle. Do not
        // start one automatically, even when the native app's reporting key is configured.
        return arguments
    }

    func start(resumeLast: Bool = true) async throws {
        let mine = generation
        try checkStartup(mine)
        let up = await isUp()
        // stop() may run while the first probe is awaiting a response, before any process
        // exists to terminate. That invalidates this startup just as task cancellation does.
        try checkStartup(mine)
        if up { return }
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
        p.arguments = Self.launchArguments(port: port, resumeLast: resumeLast,
            sentryDSN: Bundle.main.infoDictionary?["KeelSentryDSN"] as? String)
        try p.run()
        process = p

        // Poll rather than sleep: the daemon reads the repository on the way up and how long that
        // takes is a property of the repository, not a constant anyone can pick here.
        do {
            for _ in 0..<100 {
                try checkStartup(mine)
                let up = await isUp()
                try checkStartup(mine)
                if up { return }
                guard p.isRunning else {
                    throw Failure("The Keel daemon exited before answering on port \(port).")
                }
                try await Task.sleep(for: .milliseconds(50))
            }
            throw Failure("The Keel daemon did not answer on port \(port).")
        } catch {
            if process === p { stop() }
            throw error
        }
    }

    func stop() {
        generation += 1
        if let process, process.isRunning { process.terminate() }
        process = nil
    }

    private func checkStartup(_ expected: Int) throws {
        try Task.checkCancellation()
        guard generation == expected else { throw CancellationError() }
    }

    private func isUp() async -> Bool {
        var req = URLRequest(url: URL(string: "http://127.0.0.1:\(port)/api/state")!)
        req.timeoutInterval = 1
        guard let (_, response) = try? await session.data(for: req),
              let http = response as? HTTPURLResponse else { return false }
        return http.statusCode == 200
    }

    struct Failure: Error, LocalizedError {
        let message: String
        init(_ m: String) { message = m }
        var errorDescription: String? { message }
    }
}
