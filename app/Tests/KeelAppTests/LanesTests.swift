import XCTest
@testable import KeelApp

/// Which conversations were open, across a restart.
///
/// Every launch used to give you one empty lane, so whatever you were in the middle of was
/// something to go and find in History — every single time.
@MainActor
final class LanesTests: XCTestCase {
    private let repo = "/tmp/keel-test-repo"

    override func tearDown() {
        UserDefaults.standard.removeObject(forKey: "keel.lanes." + repo)
    }

    private func lanes() -> Lanes { Lanes(client: Client(port: 0), port: 0) }

    func testItRemembersTheSessionsThatWereOpen() {
        let l = lanes()
        l.lanes[0].sessionId = "aaa"
        let second = l.newLane()
        second.sessionId = "bbb"
        l.remember(repo: repo)

        let data = UserDefaults.standard.data(forKey: "keel.lanes." + repo)
        XCTAssertNotNil(data, "nothing was written for this project")

        struct Saved: Decodable { var sessions: [String]; var active: Int; var tasks: [UUID]? }
        let saved = try! JSONDecoder().decode(Saved.self, from: data!)
        XCTAssertEqual(saved.sessions, ["aaa", "bbb"])
        XCTAssertEqual(saved.active, 1, "the focused lane is remembered too")
        XCTAssertEqual(saved.tasks, l.lanes.map(\.id))
    }

    /// A lane that never ran a turn has nothing to come back to. Restoring a row of empty lanes
    /// would restore the appearance of work rather than the work.
    func testLanesWithNoSessionAreNotRemembered() {
        let l = lanes()
        l.lanes[0].sessionId = "aaa"
        _ = l.newLane()          // never sent anything
        l.remember(repo: repo)

        struct Saved: Decodable { var sessions: [String]; var active: Int }
        let data = UserDefaults.standard.data(forKey: "keel.lanes." + repo)!
        XCTAssertEqual(try! JSONDecoder().decode(Saved.self, from: data).sessions, ["aaa"])
    }

    /// Per project: "the sessions I had open" means nothing without one, and two projects must not
    /// hand each other their conversations.
    func testTheMemoryIsPerProject() {
        let l = lanes()
        l.lanes[0].sessionId = "aaa"
        l.remember(repo: repo)
        l.remember(repo: "/tmp/some-other-repo")

        XCTAssertNotNil(UserDefaults.standard.data(forKey: "keel.lanes." + repo))
        XCTAssertNotNil(UserDefaults.standard.data(forKey: "keel.lanes./tmp/some-other-repo"))
        UserDefaults.standard.removeObject(forKey: "keel.lanes./tmp/some-other-repo")
    }

    /// Nothing written for a project with no path, rather than a key called `keel.lanes.`.
    func testAnUnknownProjectWritesNothing() {
        let l = lanes()
        l.lanes[0].sessionId = "aaa"
        l.remember(repo: "")
        XCTAssertNil(UserDefaults.standard.data(forKey: "keel.lanes."))
    }

    /// A lane's checkout is remembered beside its session, and a file written before checkouts
    /// existed still decodes.
    func testACheckoutIsRememberedWithItsSession() {
        let l = lanes()
        l.lanes[0].sessionId = "aaa"
        let second = l.newLane(isolated: true)
        second.sessionId = "bbb"
        second.worktree = "fix-tests-k3f"
        l.remember(repo: repo)

        struct Saved: Decodable { var sessions: [String]; var worktrees: [String?]? }
        let data = UserDefaults.standard.data(forKey: "keel.lanes." + repo)!
        let saved = try! JSONDecoder().decode(Saved.self, from: data)
        XCTAssertEqual(saved.sessions, ["aaa", "bbb"])
        XCTAssertEqual(saved.worktrees, [nil, "fix-tests-k3f"])

        let old = #"{"sessions":["aaa"],"active":0}"#
        XCTAssertNoThrow(try JSONDecoder().decode(Saved.self, from: Data(old.utf8)))
    }

    /// A branch name from a sentence, and never an empty or hostile one.
    func testALaneIsNamedForWhatItDoes() {
        XCTAssertEqual(SessionModel.slug("Fix the billing tests!"), "fix-the-billing-tests")
        XCTAssertEqual(SessionModel.slug("../../etc"), "etc")
        XCTAssertEqual(SessionModel.slug("🚀🚀"), "lane")
        XCTAssertLessThanOrEqual(SessionModel.slug(String(repeating: "a b ", count: 40)).count, 24)
    }

    /// Closing the last lane leaves a usable window rather than an empty frame.
    func testClosingTheLastLaneStartsAFreshOne() {
        let l = lanes()
        let only = l.lanes[0]
        l.close(only)
        XCTAssertEqual(l.lanes.count, 1)
        XCTAssertNotEqual(l.lanes[0].id, only.id)
        XCTAssertNotNil(l.activeID)
    }

    func testConcurrentDetachedWindowsReserveDifferentPorts() async {
        let ports = PortReservations(first: 7778, count: 2)

        async let first = ports.reserve { _ in false }
        async let second = ports.reserve { _ in false }

        let claimed = await [first, second].compactMap { $0 }
        XCTAssertEqual(Set(claimed).count, 2)
    }

    func testAClosedDetachedWindowReleasesItsPort() async {
        let ports = PortReservations(first: 7778, count: 1)
        let first = await ports.reserve { _ in false }
        XCTAssertEqual(first, 7778)
        let exhausted = await ports.reserve { _ in false }
        XCTAssertNil(exhausted)

        await ports.release(7778)
        let reused = await ports.reserve { _ in false }
        XCTAssertEqual(reused, 7778)
    }

    func testMergeRequiresPassingIndependentProjectGate() {
        let model = SessionModel(client: Client(port: 0))
        model.isolated = true
        model.worktree = "task-123"
        let turn = Turn(prompt: "change it")
        turn.begin(call: "1", tool: "Edit", input: ["file_path": .string("src/app.swift")])
        turn.finish(call: "1", output: "ok", failed: false)
        turn.finished = true
        model.turns = [turn]

        XCTAssertEqual(model.mergeBlocker, "A project quality gate has not run.")
        turn.gate = .passed("make check", 1)
        XCTAssertNil(model.mergeBlocker)
    }

    func testMergeStopsOversizedAgentChanges() {
        let model = SessionModel(client: Client(port: 0))
        model.isolated = true
        model.worktree = "task-123"
        let turn = Turn(prompt: "change everything")
        for index in 0...8 {
            let id = "\(index)"
            turn.begin(call: id, tool: "Edit", input: ["file_path": .string("src/\(index).swift")])
            turn.finish(call: id, output: "ok", failed: false)
        }
        turn.finished = true
        turn.gate = .passed("make check", 1)
        model.turns = [turn]

        XCTAssertTrue(model.mergeBlocker?.contains("8-file budget") == true)
    }
}
