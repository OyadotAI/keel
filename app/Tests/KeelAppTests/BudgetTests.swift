import XCTest
@testable import KeelApp

/// The budgets from the plan, as tests that can fail.
///
/// Orca's most-reported structural bug is "app updates leave previous daemon generations running
/// forever — invisible agent sessions accumulate and exhaust memory", alongside a main process
/// reaching 100% CPU after churn. Keel cannot have that bug: it spawns one `claude` per turn and
/// kills it when the stream ends, and one daemon per app. But "cannot" is only true while someone
/// keeps checking, so this counts.
final class BudgetTests: XCTestCase {

    /// Processes matching `needle`, with the parent that owns each one.
    ///
    /// The parent is the whole point: a `claude` with a live parent is a turn in flight, and a
    /// `claude` whose parent is gone is the leak. Counting matches alone cannot tell them apart,
    /// which is why the first version of these tests failed on healthy machines.
    private func processes(matching needle: String) -> [(pid: Int, ppid: Int)] {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/ps")
        p.arguments = ["-Ao", "pid=,ppid=,command="]
        let pipe = Pipe()
        p.standardOutput = pipe
        guard (try? p.run()) != nil else { return [] }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()

        return String(decoding: data, as: UTF8.self)
            .split(separator: "\n")
            .filter { $0.contains(needle) && !$0.contains("ps -Ao") }
            .compactMap { line in
                let parts = line.split(separator: " ", omittingEmptySubsequences: true)
                guard parts.count >= 2, let pid = Int(parts[0]), let ppid = Int(parts[1])
                else { return nil }
                return (pid, ppid)
            }
    }

    private func processCount(matching needle: String) -> Int {
        processes(matching: needle).count
    }

    /// Orphaned: reparented to init because whoever started it is gone.
    private func orphans(matching needle: String) -> Int {
        processes(matching: needle).count { $0.ppid == 1 }
    }

    // The two ambient process-count tests that used to live here are gone. They asserted on the
    // state of the whole machine at an arbitrary instant, so they failed whenever a turn was
    // legitimately running, whenever a developer had the app open, and during the relaunch window
    // when the outgoing daemon had not yet exited — three ways to go red for a working product.
    //
    // `testTheDaemonDiesWithTheApp` below replaces both, and is the one that ever caught anything:
    // it launches the bundle, quits it, and watches. It controls the window it measures.

    /// The bundle, found by walking up: `swift test` promises nothing about the working directory.
    private func bundlePath() throws -> URL {
        var dir = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        for _ in 0..<4 {
            let candidate = dir.appendingPathComponent("dist/Keel.app")
            if FileManager.default.fileExists(atPath: candidate.path) { return candidate }
            dir = dir.deletingLastPathComponent()
        }
        throw XCTSkip("dist/Keel.app is not built; run `make app` first")
    }

    /// The daemon must not outlive the app.
    ///
    /// This is the test that matters, and the reason it launches the real bundle: the version that
    /// only counted processes *at rest* passed happily while every quit was orphaning a daemon.
    /// A budget that cannot fail is worse than no budget, because it reads as proof.
    ///
    /// macOS has no `PR_SET_PDEATHSIG`, so nothing makes this true on its own — the app has to
    /// terminate the child, and only a launch/quit cycle can show that it did.
    ///
    /// It launches the bundle's executable directly rather than through `open`, on a port of its
    /// own, and finds the daemon by *parent pid*. Every part of that is a skip this test used to
    /// take instead of running:
    ///
    /// - It counted daemons machine-wide, so it skipped whenever one was already running — which
    ///   is every developer with Keel open, i.e. everyone dogfooding, i.e. the whole audience for
    ///   the result. Owning the pid means the machine's other Keels are none of its business.
    /// - `open` hands the process to LaunchServices and returns nothing to identify it with, so
    ///   the only way to quit it again was `pkill -f`, which would also have killed the
    ///   developer's own app. `Process` gives a pid and `terminate()` sends SIGTERM to it alone —
    ///   and SIGTERM is the case worth testing, because it is one of the three that skip
    ///   `applicationWillTerminate`.
    /// - "the app did not start a daemon here" was a skip. That is the failure, not a reason to
    ///   stand down.
    func testTheDaemonDiesWithTheApp() throws {
        let bundle = try bundlePath()
        let daemonPath = bundle.appendingPathComponent("Contents/MacOS/keel").path

        // A port of its own, so this instance starts a daemon instead of attaching to the one the
        // developer already has on 7777.
        let app = Process()
        app.executableURL = bundle.appendingPathComponent("Contents/MacOS/KeelApp")
        var env = ProcessInfo.processInfo.environment
        env["KEEL_PORT"] = String(Int.random(in: 7800...7899))
        app.environment = env
        app.standardOutput = FileHandle.nullDevice
        app.standardError = FileHandle.nullDevice
        try app.run()
        defer { if app.isRunning { app.terminate() } }

        let ours = { self.processes(matching: daemonPath).filter { $0.ppid == Int(app.processIdentifier) } }

        var daemon: (pid: Int, ppid: Int)?
        for _ in 0..<60 where daemon == nil {
            Thread.sleep(forTimeInterval: 0.25)
            daemon = ours().first
        }
        guard let daemon else {
            return XCTFail("the app ran for 15s and never started a daemon of its own")
        }

        app.terminate()
        var alive = true
        for _ in 0..<40 where alive {
            Thread.sleep(forTimeInterval: 0.25)
            alive = !processes(matching: daemonPath).filter { $0.pid == daemon.pid }.isEmpty
        }
        XCTAssertFalse(alive, "daemon \(daemon.pid) outlived the app that spawned it")
    }

    private func run(_ path: String, _ args: [String]) {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: path)
        p.arguments = args
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        try? p.run()
        p.waitUntilExit()
    }

    /// The packaged app carries its resources. `Bundle.module` trapped on every machine but the
    /// one that built it, because the packaging script never copied SwiftPM's resource bundle —
    /// the crash that hit the first testers on the first click into the Designer.
    func testThePackagedAppCarriesThePicker() throws {
        let bundle = try bundlePath()
        for script in ["Picker.js", "JSONView.js"] {
            let url = bundle.appendingPathComponent("Contents/Resources/" + script)
            XCTAssertTrue(FileManager.default.fileExists(atPath: url.path),
                          "\(script) is not in the app bundle; the preview ships without it")
        }
    }

    /// Orca ships a ~250 MB DMG. The whole argument for going native is that this does not have to.
    func testBundleStaysSmall() throws {
        let bundle = try bundlePath()
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/du")
        p.arguments = ["-sm", bundle.path]
        let pipe = Pipe()
        p.standardOutput = pipe
        try p.run()
        let out = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        p.waitUntilExit()
        let megabytes = Int(out.split(separator: "\t").first ?? "0") ?? 0
        XCTAssertLessThanOrEqual(megabytes, 40, "the bundle grew to \(megabytes) MB")
    }

    /// The main page does not poll.
    ///
    /// Every `Task.sleep` on the History → session → Trace path, pinned. Each survivor is named
    /// here with its reason; a new sleep loop fails this and has to be argued in this comment
    /// before it ships. The list shrinks as the daemon's event streams land — it must never grow.
    ///
    /// Rejected: a counting `Client`. The actor has no injection seam, and adding one for a test
    /// is the abstraction the bar forbids. A source count is cruder and cannot be worked around.
    ///
    /// Survivors, as of this pin:
    /// - `SessionModel` ×8: the design-change wait (100 ms ×60, then 400 ms) — leaves with
    ///   `DesignerViewModel`; the monitors debounce (300 ms after the daemon's `monitors.changed`,
    ///   a read, not a poll); the followed-tree debounce (600 ms) — leaves once `tree.changed`
    ///   covers a lane's checkout; the watchdog (15 s, local, no request); and three fades
    ///   (`justOpened`, `remembered`, `discarded`) that are timers, not requests.
    /// - `Lanes` ×1: the daemon-startup backoff, bounded at 40 attempts.
    /// - `SidePanel` ×0: nothing. History arrives on `/api/events`.
    func testTheMainPageDoesNotPoll() throws {
        let sources = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/KeelApp")
        var sleeps: [String: Int] = [:]
        for file in ["SessionModel.swift", "Lanes.swift", "SidePanel.swift"] {
            let text = try String(contentsOf: sources.appendingPathComponent(file), encoding: .utf8)
            sleeps[file] = text.components(separatedBy: "Task.sleep").count - 1
        }
        XCTAssertEqual(sleeps, ["SessionModel.swift": 8, "Lanes.swift": 1, "SidePanel.swift": 0],
                       "a sleep loop was added to the main page; name it in this test's comment or use an event")
    }
}
