import AppKit
import SwiftUI
import XCTest
@testable import KeelApp

private final class SetupTransport: URLProtocol, @unchecked Sendable {
    static let lock = NSLock()
    nonisolated(unsafe) static var stream = ""
    nonisolated(unsafe) static var state = ""
    nonisolated(unsafe) static var paths: [String] = []

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        let path = request.url!.path
        let body = Self.lock.withLock {
            Self.paths.append(path)
            return path == "/api/claude" ? Self.state : Self.stream
        }
        let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil,
            headerFields: ["Content-Type": path == "/api/claude" ? "application/json" : "text/event-stream"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(body.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

@MainActor
final class SetupFlowTests: XCTestCase {
    private func setup(stream: String, installed: Bool = true, authenticated: Bool = false) -> ClaudeSetup {
        SetupTransport.lock.withLock {
            SetupTransport.paths = []
            SetupTransport.stream = stream
            SetupTransport.state = "{\"installed\":\(installed),\"authenticated\":\(authenticated),\"brew\":false}"
        }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [SetupTransport.self]
        return ClaudeSetup(client: Client(port: 1, configuration: config))
    }

    private func finish(_ setup: ClaudeSetup) async {
        for _ in 0..<200 where setup.busy { try? await Task.sleep(for: .milliseconds(5)) }
        XCTAssertFalse(setup.busy, "setup did not finish")
    }

    func testInstallRechecksTheBinaryBeforeOfferingLogin() async {
        let setup = setup(stream: "event: line\ndata: Installed\n\nevent: done\ndata: 0\n\n")
        setup.start(.install)
        await finish(setup)
        XCTAssertTrue(setup.status?.installed == true)
        XCTAssertFalse(setup.ready)
        XCTAssertNil(setup.failure)
        XCTAssertEqual(SetupTransport.lock.withLock { SetupTransport.paths }, ["/api/claude/install", "/api/claude"])
    }

    func testLoginVerifiesAuthenticationAfterTheBrowserFlow() async {
        let setup = setup(stream: "event: done\ndata: 0\n\n", authenticated: true)
        setup.start(.login)
        await finish(setup)
        XCTAssertTrue(setup.ready)
        XCTAssertEqual(SetupTransport.lock.withLock { SetupTransport.paths }, ["/api/claude/login", "/api/claude"])
    }

    func testAZeroExitIsNotEnoughToClaimTheUserIsSignedIn() async {
        let setup = setup(stream: "event: done\ndata: 0\n\n")
        setup.start(.login)
        await finish(setup)
        XCTAssertFalse(setup.ready)
        XCTAssertNotNil(setup.failure)
    }

    func testInstallFailureAndPrematureDisconnectAreVisible() async {
        for stream in ["event: done\ndata: 23\n\n", "event: line\ndata: Downloading\n\n"] {
            let setup = setup(stream: stream, installed: false)
            setup.start(.install)
            await finish(setup)
            XCTAssertNotNil(setup.failure)
            XCTAssertFalse(setup.ready)
        }
    }

    func testLoginLinksAreExplicitHTTPSAndDeduplicated() {
        let links = ClaudeSetup.loginLinks(in: "Open https://claude.ai/oauth/authorize?code=fixture\n"
            + "https://claude.ai/oauth/authorize?code=fixture\nfile:///etc/passwd http://insecure.test")
        XCTAssertEqual(links.count, 1)
        XCTAssertEqual(links.first?.host, "claude.ai")
    }

    func testAccountMetadataWithoutAnInstalledCLIIsNotReady() {
        let setup = ClaudeSetup(client: Client(port: 0), status: .init(installed: false,
            authenticated: true, brew: false))
        XCTAssertFalse(setup.ready)
    }

    func testPaletteHasOneNewFeatureAndOnlyOffersStopForActiveWork() {
        let model = SessionModel(client: Client(port: 0))
        let palette = Palette(model: model, open: .constant(true))
        XCTAssertEqual(palette.items.filter { $0.shortcut == "⌘N" }.count, 1)
        XCTAssertFalse(palette.items.contains { $0.title == "Stop the turn" })
        model.running = true
        XCTAssertTrue(palette.items.contains { $0.title == "Stop the turn" })
        model.running = false
    }

    func testPaletteFindsCommandsByTheirDescription() {
        let item = Palette.Item(title: "New feature", detail: "Create an isolated checkout") {}
        XCTAssertNotNil(Palette.score("checkout", item: item))
    }

    func testPaletteDoesNotBlindlyApproveQuestionsOrPlans() throws {
        let model = SessionModel(client: Client(port: 0))
        model.pending = [try JSONDecoder().decode(Wire.Pending.self, from: Data(#"{"id":"q1","tool":"AskUserQuestion","command":"","rules":[],"session_id":"fixture","input":{"questions":[]}}"#.utf8))]
        let items = Palette(model: model, open: .constant(true)).items
        XCTAssertTrue(items.contains { $0.title == "Answer the agent's question" })
        XCTAssertFalse(items.contains { $0.title.hasPrefix("Allow once:") })
        XCTAssertFalse(items.contains { $0.title.hasPrefix("Approve for this project:") })
    }
}
