import XCTest
@testable import KeelApp

/// `Turn.written(by:since:)` — the third path onto a turn's file list, and the only one that can
/// see a file written outside the checkout.
///
/// The property worth pinning is not that it parses shell: it deliberately does not. It is that a
/// path it guesses wrong costs nothing, because the mtime is what admits a candidate. Every case
/// below is a way the guess can be wrong.
@MainActor
final class ShellWriteTests: XCTestCase {
    private var dir: URL!

    override func setUpWithError() throws {
        dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("keel-shell-writes-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: dir)
    }

    @discardableResult
    private func write(_ name: String, _ body: String = "x") throws -> String {
        let url = dir.appendingPathComponent(name)
        try body.write(to: url, atomically: true, encoding: .utf8)
        return url.path
    }

    /// The case that started this: a heredoc into a path no `Edit` and no `git status` will ever
    /// mention, because it is not in the repository.
    func testFindsAHeredocTargetOutsideTheRepository() throws {
        let since = Date()
        let path = try write("plan.html")
        let command = "cat > \"\(path)\" <<'HTML'\n<p>hello</p>\nHTML"
        XCTAssertEqual(Turn.written(by: command, since: since), [path])
    }

    /// A path with a space in it, which is the one a split on whitespace loses.
    func testFindsAQuotedPathWithASpace() throws {
        let since = Date()
        let sub = dir.appendingPathComponent("My Projects")
        try FileManager.default.createDirectory(at: sub, withIntermediateDirectories: true)
        let path = sub.appendingPathComponent("notes.md").path
        try "x".write(toFile: path, atomically: true, encoding: .utf8)
        XCTAssertEqual(Turn.written(by: "printf x > '\(path)'", since: since), [path])
    }

    /// Naming a file is not writing it. The mtime is older than the call, so it is not reported —
    /// this is the whole reason the guess is allowed to be sloppy.
    func testIgnoresAFileItOnlyRead() throws {
        let path = try write("read-me.txt")
        let since = Date().addingTimeInterval(1)
        XCTAssertEqual(Turn.written(by: "grep hello \(path)", since: since), [])
    }

    /// A path that does not exist costs a `stat` and nothing else.
    func testIgnoresAPathThatIsNotThere() {
        let missing = dir.appendingPathComponent("never-written.txt").path
        XCTAssertEqual(Turn.written(by: "cat > \(missing)", since: Date().addingTimeInterval(-1)), [])
    }

    /// A directory is not a file, and `ls /some/dir` is the commonest way to name one.
    func testIgnoresADirectory() {
        XCTAssertEqual(Turn.written(by: "ls -la \(dir.path)", since: Date().addingTimeInterval(-1)), [])
    }

    /// A heredoc full of `href="https://…"` is a thousand things that look like paths. None of
    /// them are, and the cap means none of them are stat'ed either.
    func testIgnoresURLs() {
        let body = (0..<50).map { "<a href=\"https://example.com/page/\($0)\">x</a>" }.joined()
        XCTAssertEqual(Turn.written(by: "cat > /nope/x.html <<'H'\n\(body)\nH", since: Date()), [])
    }

    /// One file named twice in one command is one row.
    func testReportsEachFileOnce() throws {
        let since = Date()
        let path = try write("twice.txt")
        let command = "printf a > \(path) && printf b >> \(path)"
        XCTAssertEqual(Turn.written(by: command, since: since), [path])
    }

    /// A `Bash` call is not the only way onto the list, and the other one must keep working:
    /// `Edit` still reports through `writeTools`, and the two arrive in the order they happened.
    ///
    /// The file is written *between* `begin` and `finish` on purpose. `Call.started` is stamped
    /// when Keel sees the call open, and the command runs after that — a file already on disk when
    /// the call began was not written by it, which is exactly what the mtime is asked to decide.
    func testATurnCollectsShellWritesAlongsideEdits() throws {
        let path = dir.appendingPathComponent("scratch.json").path
        let turn = Turn(prompt: "write something")
        turn.begin(call: "1", tool: "Write", input: ["file_path": .string("/repo/src/main.rs")])
        turn.begin(call: "2", tool: "Bash", input: ["command": .string("cat > \(path) <<'J'\n{}\nJ")])
        try "{}".write(toFile: path, atomically: true, encoding: .utf8)
        turn.finish(call: "2", output: "", failed: false)
        XCTAssertEqual(turn.files, ["/repo/src/main.rs", path])
    }

    /// The same call, with the file already on disk before it started: nothing is attributed.
    func testDoesNotClaimAFileThatPredatesTheCall() throws {
        let path = try write("already-there.json")
        let turn = Turn(prompt: "look at something")
        turn.begin(call: "1", tool: "Bash", input: ["command": .string("wc -l \(path)")])
        turn.finish(call: "1", output: "3", failed: false)
        XCTAssertEqual(turn.files, [])
    }
}
