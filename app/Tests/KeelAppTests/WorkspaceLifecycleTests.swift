import XCTest
@testable import KeelApp

private final class WorkspaceTransport: URLProtocol, @unchecked Sendable {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var requests: [URLRequest] = []

    static func reset() { lock.withLock { requests.removeAll() } }
    static func received(_ path: String, worktree: String?) -> Int {
        lock.withLock {
            requests.count {
                guard let url = $0.url, url.path == path else { return false }
                return URLComponents(url: url, resolvingAgainstBaseURL: false)?
                    .queryItems?.first(where: { $0.name == "wt" })?.value == worktree
            }
        }
    }
    static func query(_ path: String) -> [String: String]? {
        lock.withLock {
            guard let url = requests.last(where: { $0.url?.path == path })?.url else { return nil }
            let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
            return Dictionary(uniqueKeysWithValues: items.map { ($0.name, $0.value ?? "") })
        }
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        Self.lock.withLock { Self.requests.append(request) }
        let url = request.url!
        let isolated = URLComponents(url: url, resolvingAgainstBaseURL: false)?
            .queryItems?.contains(where: { $0.name == "wt" && $0.value == "feature" }) == true
        let branch = isolated ? "keel/updated-feature" : "main"
        let body: String
        switch url.path {
        case "/api/events": body = "event: connected\ndata: {}\n\n"
        case "/api/worktree":
            body = #"[{"name":"feature","branch":"keel/feature","path":"/tmp/feature","ahead":1,"dirty":true}]"#
        case "/api/git/status":
            body = #"{"is_repo":true,"branch":"\#(branch)","changes":[],"repos":[],"collapsed":false}"#
        case "/api/git/branches":
            body = #"{"current":"\#(branch)","local":[],"remote":[],"remotes":[],"staged":0,"unstaged":0}"#
        case "/api/tree":
            body = #"[{"name":"file.swift","path":"\#(isolated ? "feature" : "root")/file.swift","dir":false}]"#
        default: body = "[]"
        }
        let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1",
                                       headerFields: ["Content-Type": url.path == "/api/events"
                                                      ? "text/event-stream" : "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(body.utf8))
        if url.path != "/api/events" { client?.urlProtocolDidFinishLoading(self) }
    }
    override func stopLoading() {}
}

private final class DaemonProbeTransport: URLProtocol, @unchecked Sendable {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var pending: DaemonProbeTransport?
    static var received: DaemonProbeTransport? { lock.withLock { pending } }
    static func reset() { lock.withLock { pending = nil } }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() { Self.lock.withLock { Self.pending = self } }
    override func stopLoading() {}

    func answer() {
        let response = HTTPURLResponse(url: request.url!, statusCode: 200,
                                       httpVersion: "HTTP/1.1", headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data("{}".utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
}

@MainActor
final class WorkspaceLifecycleTests: XCTestCase {
    private func client() -> Client {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [WorkspaceTransport.self]
        return Client(port: 0, configuration: configuration)
    }

    private func eventually(_ condition: () -> Bool) async -> Bool {
        for _ in 0..<100 {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(5))
        }
        return condition()
    }

    func testEventSubscriptionDoesNotKeepAnUnusedWindowAlive() async {
        var lanes: Lanes? = Lanes(client: Client(port: 0), port: 0)
        weak var releasedLanes = lanes
        weak var releasedProject = lanes?.project
        // Let the event task enter its waiting loop before its owner goes away.
        for _ in 0..<10 { await Task.yield() }
        lanes = nil

        let released = await eventually { releasedLanes == nil && releasedProject == nil }
        XCTAssertTrue(released, "an idle project subscription retained the entire closed window")
        // Also clean up the deliberately failing pre-fix case.
        releasedProject?.events.stop()
    }

    func testStoppingTheEventBusFinishesEverySubscriber() async {
        let events = DaemonEvents()
        let streams = [events.subscribe(), events.subscribe()]
        var finished = 0
        let readers = streams.map { stream in
            Task { @MainActor in
                for await _ in stream {}
                finished += 1
            }
        }
        defer { for reader in readers { reader.cancel() } }

        events.stop()

        let ended = await eventually { finished == streams.count }
        XCTAssertTrue(ended, "stopping transport left all subscriber tasks suspended forever")
    }

    func testDetachedWorkspaceShutdownStopsFollowingItsTranscript() {
        let workspace = Workspace(port: 0)
        let lane = workspace.lanes.active
        lane.following = true
        lane.replaying = true

        workspace.shutdown()

        XCTAssertFalse(lane.following)
        XCTAssertFalse(lane.replaying)
    }

    func testAppShutdownStopsFollowingItsMainTranscript() {
        let app = AppModel(port: 0)
        let lane = app.lanes.active
        lane.following = true
        lane.replaying = true

        app.shutdown()

        XCTAssertFalse(lane.following)
        XCTAssertFalse(lane.replaying)
    }

    func testStoppedEventConnectionCannotRemainConnected() async {
        let events = DaemonEvents()
        events.start(client())
        defer { events.stop() }
        let connected = await eventually { events.connected }
        XCTAssertTrue(connected)

        events.stop()

        XCTAssertFalse(events.connected, "the window advertised a connection it had shut down")
    }

    func testReconnectRefreshesEachOpenCheckoutOnce() async {
        WorkspaceTransport.reset()
        let lanes = Lanes(client: client(), port: 0)
        lanes.active.sessionId = "root"
        let first = lanes.newLane(resuming: "first", isolated: true)
        first.worktree = "feature"
        let second = lanes.newLane(resuming: "second", isolated: true)
        second.worktree = "feature"
        let feature = lanes.project.repo(for: "feature")
        feature.branch = "stale"

        await lanes.route(Wire.ProjectEvent(kind: "connected", wt: nil, seq: 0, json: "{}"))

        XCTAssertEqual(lanes.project.repo(for: nil).branch, "main")
        XCTAssertEqual(feature.branch, "keel/updated-feature")
        XCTAssertEqual(feature.files, ["feature/file.swift"])
        XCTAssertEqual(feature.branches?.current, "keel/updated-feature")
        for path in ["/api/git/status", "/api/tree", "/api/git/branches"] {
            XCTAssertEqual(WorkspaceTransport.received(path, worktree: nil), 1, path)
            XCTAssertEqual(WorkspaceTransport.received(path, worktree: "feature"), 1, path)
        }
    }

    func testStoppingDaemonDuringItsProbeInvalidatesStartup() async throws {
        DaemonProbeTransport.reset()
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [DaemonProbeTransport.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel(); DaemonProbeTransport.reset() }
        let daemon = Daemon(port: 0, session: session)
        let start = Task { try await daemon.start(resumeLast: false) }
        defer { start.cancel(); daemon.stop() }
        let probing = await eventually { DaemonProbeTransport.received != nil }
        XCTAssertTrue(probing)
        let probe = try XCTUnwrap(DaemonProbeTransport.received)

        daemon.stop()
        probe.answer()

        do {
            try await start.value
            XCTFail("startup succeeded after the window stopped its daemon")
        } catch is CancellationError {
            // A stop invalidates the result, even if the successful probe lands afterward.
        }
    }

    func testDetachedCheckoutKeepsItsIdentityAndAgentPreferences() throws {
        let savedModel = UserDefaults.standard.object(forKey: "keel.model")
        defer { UserDefaults.standard.set(savedModel, forKey: "keel.model") }
        let source = SessionModel(client: client(), port: 0)
        source.repoPath = "/tmp/project"
        source.sessionId = "feature-session"
        source.worktree = "feature"
        source.isolated = true
        source.provider = .codex
        source.claudeModel = "chosen-model"
        source.mode = "plan"
        source.baseBranch = "release"
        source.chosenName = "feature-name"
        source.title = "Keep the feature"
        source.sessionCwd = "/tmp/project/.keel/worktrees/feature"
        let payload = try JSONEncoder().encode(Detached(model: source))
        let request = try JSONDecoder().decode(Detached.self, from: payload)

        let workspace = Workspace(port: 0, request: request)
        defer { workspace.shutdown() }
        let moved = workspace.lanes.active

        XCTAssertEqual(moved.id, source.id)
        XCTAssertEqual(moved.sessionId, source.sessionId)
        XCTAssertEqual(moved.q()["wt"], "feature")
        XCTAssertEqual(moved.repo.worktree, source.repo.worktree)
        XCTAssertEqual(moved.sessionCwd, source.sessionCwd)
        XCTAssertTrue(moved.isolated)
        XCTAssertEqual(moved.provider, .codex)
        XCTAssertEqual(moved.claudeModel, "chosen-model")
        XCTAssertEqual(moved.mode, "plan")
        XCTAssertEqual(moved.baseBranch, "release")
        XCTAssertEqual(moved.chosenName, "feature-name")
        XCTAssertEqual(moved.title, "Keep the feature")
        XCTAssertFalse(workspace.lanes.remembers)
    }

    func testOldDetachedWindowPayloadStillDecodesWithSafeDefaults() throws {
        let id = UUID()
        let payload = Data(#"{"lane":"\#(id.uuidString)","project":"/tmp/project","session":"old-session","title":"Old feature"}"#.utf8)
        let request = try JSONDecoder().decode(Detached.self, from: payload)
        let workspace = Workspace(port: 0, request: request)
        defer { workspace.shutdown() }

        XCTAssertEqual(workspace.lanes.active.id, id)
        XCTAssertEqual(workspace.lanes.active.sessionId, "old-session")
        XCTAssertEqual(workspace.lanes.active.provider, .claude)
        XCTAssertEqual(workspace.lanes.active.mode, "acceptEdits")
        XCTAssertNil(workspace.lanes.active.worktree)
        XCTAssertFalse(workspace.lanes.active.isolated)
    }

    func testDetachedTranscriptAndFileQueriesUseTheSameCheckout() async {
        WorkspaceTransport.reset()
        let request = Detached(lane: UUID(), project: "/tmp/project", session: "resumed",
                               title: "Feature", worktree: "feature", isolated: true,
                               mode: "plan", sessionCwd: "/tmp/project/old-checkout")
        let moved = SessionModel(client: client(), port: 0)
        defer { moved.closed() }
        request.configure(moved)
        moved.sessions = [Wire.Session(id: "resumed", title: "Feature", messages: 1,
                                       lastActive: nil, scope: "below",
                                       cwd: "/tmp/project/old-checkout", live: false, busy: false)]

        await moved.open(session: "resumed", cwd: request.resumeCwd)

        let followed = await eventually { WorkspaceTransport.query("/api/session/tail") != nil }
        XCTAssertTrue(followed)
        let tail = WorkspaceTransport.query("/api/session/tail")
        XCTAssertEqual(moved.q()["wt"], "feature")
        XCTAssertEqual(tail?["wt"], "feature")
        XCTAssertEqual(tail?["cwd"], "/tmp/project/.keel/worktrees/feature")
        XCTAssertEqual(moved.mode, "plan")
    }
}
