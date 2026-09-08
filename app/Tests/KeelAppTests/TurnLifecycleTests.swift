import AppKit
import Foundation
import XCTest
@testable import KeelApp

/// Hold HTTP responses at the actual suspension points in the lane lifecycle.
private final class TurnTransport: URLProtocol, @unchecked Sendable {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var requests: [TurnTransport] = []
    nonisolated(unsafe) private static var draining = false
    nonisolated(unsafe) private static var holdMonitors = false
    private var answered = false

    static func reset() { lock.withLock { requests = []; draining = false; holdMonitors = false } }

    static func holdBackgroundJobs() { lock.withLock { holdMonitors = true } }

    static func finishPending() {
        let pending = lock.withLock { draining = true; return requests }
        for request in pending where request.request.url?.path != "/api/chat" {
            request.reply("{}")
        }
    }

    static func received(_ path: String) -> [TurnTransport] {
        lock.withLock { requests.filter { $0.request.url?.path == path } }
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.lock.withLock { Self.requests.append(self) }
        if Self.lock.withLock({ Self.draining }) { reply("{}"); return }
        if request.url?.path == "/api/monitors", Self.lock.withLock({ Self.holdMonitors }) { return }
        switch request.url?.path {
        case "/api/chat", "/api/chat/stop", "/api/approve/poll", "/api/approve/answer":
            break // The test decides when these replies arrive.
        case "/api/monitors", "/api/tree", "/api/git/log", "/api/worktrees":
            reply("[]")
        default:
            reply("{}")
        }
    }

    override func stopLoading() {}

    func reply(_ json: String) {
        guard Self.lock.withLock({
            if answered { return false }
            answered = true
            return true
        }) else { return }
        let response = HTTPURLResponse(url: request.url!, statusCode: 200,
                                       httpVersion: "HTTP/1.1",
                                       headerFields: ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(json.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }

    func fail() { client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost)) }
}

@MainActor
final class TurnLifecycleTests: XCTestCase {
    private var models: [SessionModel] = []

    override func setUp() { TurnTransport.reset() }

    override func tearDown() async throws {
        await MainActor.run {
            for model in models { model.closed() }
            models = []
        }
        TurnTransport.finishPending()
        try await Task.sleep(for: .milliseconds(50))
    }

    private func model() -> SessionModel {
        _ = NSApplication.shared
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [TurnTransport.self]
        let model = SessionModel(client: Client(port: 0, configuration: config), port: 0)
        model.policyRequiresIsolation = false
        models.append(model)
        return model
    }

    private func waitFor(_ path: String, count: Int = 1) async throws -> TurnTransport {
        for _ in 0..<200 {
            let requests = TurnTransport.received(path)
            if requests.count >= count { return requests[count - 1] }
            try await Task.sleep(for: .milliseconds(10))
        }
        XCTFail("Never received request \(count) for \(path)")
        throw URLError(.timedOut)
    }

    func testASendDuringTurnCleanupQueuesInOrder() {
        let model = model()
        let finished = Turn(prompt: "first")
        finished.finished = true
        model.turns = [finished]
        model.settling = true
        model.queued = ["second"]
        model.prompt = "third"

        model.send()

        XCTAssertEqual(model.queued, ["second", "third"])
        XCTAssertTrue(model.current === finished)
        XCTAssertFalse(model.running)
    }

    func testACancelledStreamCannotFinishTheReplacementTurn() async throws {
        let model = model()
        model.prompt = "first"
        model.send()
        _ = try await waitFor("/api/chat")

        model.stop()
        model.prompt = "replacement"
        model.send()
        let replacement = try XCTUnwrap(model.current)
        let stop = try await waitFor("/api/chat/stop")
        stop.reply(#"{"stopped":true}"#)
        _ = try await waitFor("/api/chat", count: 2)

        // Flush the cancelled stream's completion and its fast repository refreshes.
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertTrue(model.current === replacement)
        XCTAssertTrue(model.running, "cleanup for the cancelled stream marked its replacement idle")
        XCTAssertFalse(replacement.finished)
        XCTAssertNil(model.lastError, "cancellation must not become a failure on the new turn")
    }

    func testAReplacementWaitsUntilTheStopRequestHasReturned() async throws {
        let model = model()
        model.prompt = "first"
        model.send()
        _ = try await waitFor("/api/chat")
        model.stop()
        let stop = try await waitFor("/api/chat/stop")

        model.prompt = "replacement"
        model.send()
        // A delayed lane-scoped Stop must arrive before another chat can take that lane.
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(TurnTransport.received("/api/chat").count, 1)
        stop.reply(#"{"stopped":true}"#)
        _ = try await waitFor("/api/chat", count: 2)
        XCTAssertTrue(model.running)
    }

    func testAnApprovalFetchedBeforeStopDoesNotReappearAfterIt() async throws {
        let model = model()
        model.turns = [Turn(prompt: "first")]
        model.running = true
        let fetch = Task { await model.fetchApprovals() }
        let poll = try await waitFor("/api/approve/poll")
        model.stop()
        poll.reply(#"[{"id":"old-question","tool":"Bash","command":"make check","rules":[],"session_id":"s"}]"#)
        await fetch.value

        XCTAssertTrue(model.pending.isEmpty, "a late poll resurrected an approval after Stop")
        // Poll drains the daemon queue, so this response must release its waiter too.
        let answer = try await waitFor("/api/approve/answer")
        answer.reply(#"{"ok":true}"#)
    }

    func testAFailedAnswerDoesNotReopenAStoppedTurnsApproval() async throws {
        let model = model()
        model.turns = [Turn(prompt: "first")]
        model.running = true
        let pending = try JSONDecoder().decode(Wire.Pending.self, from: Data(
            #"{"id":"old-question","tool":"Bash","command":"make check","rules":[],"session_id":"s"}"#.utf8))
        model.pending = [pending]
        model.answer(pending, text: "yes")
        let answer = try await waitFor("/api/approve/answer")
        model.stop()
        answer.fail()
        try await Task.sleep(for: .milliseconds(100))

        XCTAssertTrue(model.pending.isEmpty)
        XCTAssertFalse(model.lastError?.contains("still waiting") == true)
    }

    func testAJobResponseCannotStartAnAgentInAClosedLane() async throws {
        TurnTransport.holdBackgroundJobs()
        let model = model()
        let jobs = try await waitFor("/api/monitors")
        model.closed()
        jobs.reply(#"[{"id":"job-1","lane":"lane","command":"make check","dir":".","started":1,"finished":2,"exit":0,"log":["passed"],"reported":false}]"#)
        try await Task.sleep(for: .milliseconds(100))

        XCTAssertFalse(model.running)
        XCTAssertTrue(model.turns.isEmpty, "a late background job reopened a closed lane")
        XCTAssertTrue(TurnTransport.received("/api/monitors/ack").isEmpty,
                      "a closed reader must leave the result available to its next reader")
    }

    func testATransportFailureDoesNotStartAFreshAgent() async throws {
        let model = model()
        model.prompt = "first"
        model.send()
        let chat = try await waitFor("/api/chat")
        chat.fail()
        for _ in 0..<200 {
            if !model.running && !model.settling { break }
            try await Task.sleep(for: .milliseconds(10))
        }

        XCTAssertFalse(model.running)
        XCTAssertNotNil(model.current?.failure)
        XCTAssertTrue(model.current?.failed == true)
        XCTAssertEqual(TurnTransport.received("/api/chat").count, 1,
                       "a network failure is not evidence that the conversation is missing")
    }
}
