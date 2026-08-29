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

        struct Saved: Decodable { var sessions: [String]; var active: Int }
        let saved = try! JSONDecoder().decode(Saved.self, from: data!)
        XCTAssertEqual(saved.sessions, ["aaa", "bbb"])
        XCTAssertEqual(saved.active, 1, "the focused lane is remembered too")
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
}
