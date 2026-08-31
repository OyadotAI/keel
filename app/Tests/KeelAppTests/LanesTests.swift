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

/// Paths as the checkout serving the diff knows them.
///
/// A lane writes to `<project>/.keel/worktrees/<lane>/…` and its diff is answered from that
/// checkout, so the prefix has to come off. It did not: the project root was stripped first,
/// leaving `.keel/worktrees/<lane>/…`, and the marker that should have removed it wanted a
/// leading slash. Every file a lane touched then asked git for a path that was not there, and
/// the diff pane said the file matched HEAD — a lane's work could not be reviewed at all.
@MainActor
final class LanePathTests: XCTestCase {
    private func model(worktree: String?) -> SessionModel {
        let m = SessionModel(client: Client(port: 0))
        m.repoPath = "/Users/x/proj"
        m.worktree = worktree
        return m
    }

    func testALanesFileIsRelativeToItsOwnCheckout() {
        let m = model(worktree: "fix-ci-ad0")
        XCTAssertEqual(
            m.repoRelative("/Users/x/proj/.keel/worktrees/fix-ci-ad0/.github/dependabot.yml"),
            ".github/dependabot.yml")
    }

    func testTheProjectsOwnFilesAreUnaffected() {
        XCTAssertEqual(model(worktree: nil).repoRelative("/Users/x/proj/src/main.rs"), "src/main.rs")
        XCTAssertEqual(model(worktree: "lane").repoRelative("/Users/x/proj/src/main.rs"), "src/main.rs")
    }

    /// Another lane's checkout is not this one's, so its prefix is not stripped by this lane.
    func testOnlyThisLanesPrefixComesOff() {
        let m = model(worktree: "mine")
        XCTAssertEqual(m.repoRelative("/Users/x/proj/.keel/worktrees/theirs/a.txt"),
                       ".keel/worktrees/theirs/a.txt")
    }
}

/// Nothing typed is lost when a turn ends without running.
///
/// `endTurn` is the only place that drains `queued`, and Stop does not go through it. The composer
/// kept saying "they run in order when this turn ends" about a turn that had ended, and the next
/// send — taken as not-running — called `start` directly and stepped over them permanently.
@MainActor
final class QueuedTests: XCTestCase {
    private func lane() -> SessionModel { SessionModel(client: Client(port: 0), port: 0) }

    func testStopGivesQueuedMessagesBackToTheComposer() {
        let m = lane()
        m.running = true
        m.prompt = "first"; m.send()
        m.prompt = "second"; m.send()
        XCTAssertEqual(m.queued, ["first", "second"], "both should be waiting on the turn")

        m.stop()
        XCTAssertTrue(m.queued.isEmpty, "the queue must not outlive the turn it was waiting on")
        XCTAssertEqual(m.prompt, "first\n\nsecond")
    }

    func testWhatWasHalfTypedKeepsItsPlaceAfterThem() {
        let m = lane()
        m.running = true
        m.prompt = "queued one"; m.send()
        m.prompt = "still typing this"

        m.stop()
        XCTAssertEqual(m.prompt, "queued one\n\nstill typing this")
    }

    func testAnEmptyQueueLeavesTheComposerAlone() {
        let m = lane()
        m.running = true
        m.prompt = "untouched"
        m.stop()
        XCTAssertEqual(m.prompt, "untouched")
    }
}

/// Two lanes must not write one working tree.
///
/// Auto-commit is `git add -A` in the checkout, so whichever turn ends first commits the other's
/// half-written files under its own prompt. Reproduced against the daemon before this guard: a
/// commit named "lane A: add a.txt" containing lane B's b.txt.
@MainActor
final class SharedTreeTests: XCTestCase {
    private func lanes() -> Lanes { Lanes(client: Client(port: 0), port: 0) }

    func testASecondWritingLaneIsRefusedRatherThanRun() {
        let l = lanes()
        let first = l.lanes[0]
        first.title = "the one already editing"
        first.running = true

        let second = l.newLane()
        second.prompt = "change something else"
        second.send()

        XCTAssertFalse(second.running, "it must not start beside a lane writing the same tree")
        XCTAssertNotNil(second.lastError)
        XCTAssertTrue(second.lastError!.contains("the one already editing"), second.lastError!)
        XCTAssertEqual(second.lastFix?.label, "Give this one its own branch")
    }

    func testALaneWithItsOwnBranchIsNotBlocked() {
        let l = lanes()
        l.lanes[0].running = true
        XCTAssertNotNil(l.writingElsewhere(than: l.newLane()))

        let isolated = l.newLane(isolated: true)
        XCTAssertNotNil(l.writingElsewhere(than: isolated),
                        "the other lane is still writing; it is this lane's isolation that saves it")
        isolated.prompt = "on my own branch"
        isolated.send()
        XCTAssertTrue(isolated.queued.isEmpty && isolated.lastError == nil)
    }

    /// A plan turn writes nothing, so it is never the lane anyone has to wait for.
    func testAPlanningLaneIsNotWriting() {
        let l = lanes()
        l.lanes[0].running = true
        l.lanes[0].mode = "plan"
        XCTAssertNil(l.writingElsewhere(than: l.newLane()))
    }
}


/// One agent per lane, kept by the function that starts them.
@MainActor
final class OneTurnTests: XCTestCase {
    /// `retryLast` guards `running` itself, so this asserts the guard *underneath* it: a lane that
    /// is already running does not get a second provider process, whichever path asked for one.
    func testARunningLaneQueuesInsteadOfStartingASecondAgent() {
        let m = SessionModel(client: Client(port: 0), port: 0)
        m.running = true
        m.prompt = "the second thing"
        m.send()

        XCTAssertEqual(m.queued, ["the second thing"])
        XCTAssertEqual(m.turns.count, 0, "no turn was opened for it")
        XCTAssertTrue(m.running, "the turn already in flight is untouched")
    }

    /// And the ordinary path still runs: the guard is about `running`, not about being cautious.
    func testAnIdleLaneStarts() {
        let m = SessionModel(client: Client(port: 0), port: 0)
        m.prompt = "the first thing"
        m.send()
        XCTAssertTrue(m.running)
        XCTAssertEqual(m.turns.count, 1)
        XCTAssertTrue(m.queued.isEmpty)
        m.stop()
    }
}

/// A lane that leaves the window takes everything it was running with it.
@MainActor
final class LaneShutdownTests: XCTestCase {
    /// `newLane` hands back the spare empty lane rather than making a second one, so a test that
    /// wants two lanes has to give the first a conversation first. Learned by writing it wrong.
    private func twoLanes() -> (Lanes, SessionModel, SessionModel) {
        let l = Lanes(client: Client(port: 0), port: 0)
        let first = l.lanes[0]
        first.sessionId = "already-talking"
        let second = l.newLane()
        XCTAssertNotEqual(first.id, second.id, "the second lane is a second lane")
        return (l, first, second)
    }

    func testClosingALaneEndsItsBackgroundWatch() {
        let (l, first, second) = twoLanes()
        second.watchMonitors()
        XCTAssertTrue(second.isWatchingMonitors, "the loop is up")

        l.close(second)
        XCTAssertFalse(second.isWatchingMonitors, "closing the lane must end it")
        XCTAssertEqual(l.lanes.map(\.id), [first.id])
    }

    /// Switching project drops every lane but the one that did the switching, and the same
    /// applies: their loops were polling for jobs in a project that is no longer open.
    func testSwitchingProjectEndsTheDroppedLanesWatches() async {
        let (l, dropped, keep) = twoLanes()
        dropped.watchMonitors()
        l.activeID = keep.id

        await l.switchProject(to: "/tmp/keel-test-other")
        XCTAssertFalse(dropped.isWatchingMonitors)
        XCTAssertEqual(l.lanes.map(\.id), [keep.id])
    }
}

/// The remembered slash commands are read back, not only written.
///
/// `loadCommands` said in its own doc comment that it existed "so the picker works before the
/// first turn of a session, which is exactly when somebody reaches for `/`" — and nothing called
/// it. The list was written to `UserDefaults` after every turn and never read, so `/` in a freshly
/// opened project offered Keel's own single command and nothing else.
@MainActor
final class SlashCommandTests: XCTestCase {
    private let repo = "/tmp/keel-test-commands"
    private var key: String { "keel.slashCommands." + repo }

    override func setUp() {
        UserDefaults.standard.set(["/review", "/deploy"], forKey: key)
    }
    override func tearDown() {
        UserDefaults.standard.removeObject(forKey: key)
    }

    func testOpeningAProjectRestoresItsRememberedCommands() {
        let m = SessionModel(client: Client(port: 0), port: 0)
        XCTAssertTrue(m.slashCommands.isEmpty, "nothing is known before the project is")

        m.repoPath = repo
        m.loadCommands()
        XCTAssertEqual(m.slashCommands, ["/review", "/deploy"])
    }

    /// What arrived from a running session wins; the cache is only for before that.
    func testALiveListIsNotOverwrittenByTheCache() {
        let m = SessionModel(client: Client(port: 0), port: 0)
        m.repoPath = repo
        m.slashCommands = ["/from-this-session"]
        m.loadCommands()
        XCTAssertEqual(m.slashCommands, ["/from-this-session"])
    }

    /// A second lane in the same window starts with what the first one knows.
    func testANewLaneAdoptsTheCommandsTheProjectAlreadyHas() {
        let l = Lanes(client: Client(port: 0), port: 0)
        l.lanes[0].sessionId = "already-talking"
        l.lanes[0].slashCommands = ["/review"]
        XCTAssertEqual(l.newLane().slashCommands, ["/review"])
    }

    /// A new tab holds the project's data the moment it is made, so it must not also hold "not
    /// read yet" — that combination is a panel spinning over a list it already has.
    func testANewLaneIsNotStillReading() {
        let l = Lanes(client: Client(port: 0), port: 0)
        let first = l.lanes[0]
        first.sessionId = "already-talking"   // or `+` just reuses this empty lane
        first.loaded = true
        first.sessions = [Wire.Session(id: "aaa", title: "a", messages: 1)]

        let second = l.newLane()
        XCTAssertEqual(second.sessions.count, 1, "the new lane took the project's sessions")
        XCTAssertTrue(second.loaded, "and must not draw Reading… over them")
    }
}
