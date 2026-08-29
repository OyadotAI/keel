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
    func testTheDaemonDiesWithTheApp() throws {
        let bundle = try bundlePath()
        let marker = bundle.appendingPathComponent("Contents/MacOS/keel").path
        try XCTSkipUnless(processCount(matching: marker) == 0,
                          "a bundled daemon is already running; not this test's to judge")

        run("/usr/bin/open", [bundle.path])
        defer { run("/usr/bin/pkill", ["-f", bundle.appendingPathComponent("Contents/MacOS/KeelApp").path]) }

        var started = false
        for _ in 0..<40 where !started {
            Thread.sleep(forTimeInterval: 0.25)
            started = processCount(matching: marker) > 0
        }
        try XCTSkipUnless(started, "the app did not start a daemon here")

        run("/usr/bin/pkill", ["-f", bundle.appendingPathComponent("Contents/MacOS/KeelApp").path])

        var survivors = 1
        for _ in 0..<20 where survivors > 0 {
            Thread.sleep(forTimeInterval: 0.25)
            survivors = processCount(matching: marker)
        }
        XCTAssertEqual(survivors, 0, "the daemon outlived the app that spawned it")
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
        let picker = bundle.appendingPathComponent("Contents/Resources/Picker.js")
        XCTAssertTrue(FileManager.default.fileExists(atPath: picker.path),
                      "Picker.js is not in the app bundle; the Designer will crash on open")
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
}
