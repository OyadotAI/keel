import AppKit
import SwiftUI
import XCTest
@testable import KeelApp

private final class JourneyTransport: URLProtocol, @unchecked Sendable {
    static let lock = NSLock()
    nonisolated(unsafe) static var response = "{}"
    nonisolated(unsafe) static var code = 200
    nonisolated(unsafe) static var requests: [URLRequest] = []
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        let (body, status) = Self.lock.withLock {
            Self.requests.append(request)
            return (Self.response, Self.code)
        }
        client?.urlProtocol(self, didReceive: HTTPURLResponse(url: request.url!, statusCode: status,
            httpVersion: nil, headerFields: ["Content-Type": "application/json"])!, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(body.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

@MainActor
final class JourneyTests: XCTestCase {
    private let contexts = #"{"current-context":"dev","contexts":[{"name":"dev","context":{"cluster":"local","namespace":""}},{"name":"staging","context":{"cluster":"staging-cluster","namespace":"apps"}}]}"#

    private func client(response: String, status: Int = 200) -> Client {
        JourneyTransport.lock.withLock {
            JourneyTransport.response = response
            JourneyTransport.code = status
            JourneyTransport.requests = []
        }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [JourneyTransport.self]
        return Client(port: 1, configuration: config)
    }

    func testContextLoadSelectApplyAndConfirmation() async {
        let setup = KubernetesSetup(client: client(response: contexts))
        await setup.refresh()
        XCTAssertEqual(setup.contexts.count, 2)
        XCTAssertEqual(setup.selected, "dev")
        XCTAssertFalse(setup.canApply)
        XCTAssertEqual(JourneyTransport.lock.withLock { JourneyTransport.requests.map(\.httpMethod) }, ["GET"])
        setup.selected = "staging"
        XCTAssertTrue(setup.canApply)
        // Choosing a row has not mutated anything.
        XCTAssertEqual(JourneyTransport.lock.withLock { JourneyTransport.requests.count }, 1)
        JourneyTransport.lock.withLock {
            JourneyTransport.response = contexts.replacingOccurrences(of: #""current-context":"dev""#, with: #""current-context":"staging""#)
        }
        await setup.apply()
        XCTAssertEqual(setup.current, "staging")
        XCTAssertNotNil(setup.confirmation)
        XCTAssertNil(setup.failure)
        XCTAssertFalse(setup.canApply)
        XCTAssertEqual(JourneyTransport.lock.withLock { JourneyTransport.requests.map(\.httpMethod) }, ["GET", "POST"])
    }

    func testContextFailureCanRetryAndDoesNotPretendItWorked() async {
        let setup = KubernetesSetup(client: client(response: contexts))
        await setup.refresh()
        setup.selected = "staging"
        JourneyTransport.lock.withLock { JourneyTransport.code = 400; JourneyTransport.response = "permission denied" }
        await setup.apply()
        XCTAssertTrue(setup.failure?.contains("permission denied") == true)
        XCTAssertNil(setup.confirmation)
        XCTAssertFalse(setup.applying)
        XCTAssertEqual(setup.current, "dev")
        JourneyTransport.lock.withLock { JourneyTransport.code = 200; JourneyTransport.response = contexts }
        await setup.apply() // HTTP 200 with unchanged state is not success.
        XCTAssertTrue(setup.failure?.contains("not saved") == true)
        XCTAssertNil(setup.confirmation)
    }

    func testEmptyAndStaleContextsCannotBeApplied() async {
        let setup = KubernetesSetup(client: client(response: #"{"contexts":null}"#))
        await setup.refresh()
        XCTAssertTrue(setup.loaded)
        XCTAssertTrue(setup.contexts.isEmpty)
        setup.selected = "vanished"
        XCTAssertFalse(setup.canApply)
        await setup.apply()
        XCTAssertEqual(JourneyTransport.lock.withLock { JourneyTransport.requests.count }, 1)
    }

    func testConnectionLabelsDoNotCallLocalDaemonsSignIns() {
        for id in ["docker", "kubectl", "tailscale"] {
            let tool = ConnectionsSettings.Tool(id: id, label: id, installed: true, authenticated: false)
            XCTAssertEqual(tool.statusLabel, "SET UP")
        }
    }

    func testSelectedKubernetesContextIsNotPresentedAsMissingSetup() {
        let tool = ConnectionsSettings.Tool(id: "kubectl", label: "Kubernetes", installed: true,
                                            authenticated: false, identity: "staging")
        XCTAssertEqual(tool.statusLabel, "CHECK")
        var expired = tool
        expired.reconnect = "gcloud"
        XCTAssertEqual(expired.statusLabel, "SIGN IN")
        XCTAssertEqual(expired.identity, "staging")
    }

    func testToolRefreshUpdatesSharedWarningBadgeAndClearsAfterRecovery() async {
        let failed = #"[{"id":"kubectl","label":"Kubernetes","installed":true,"authenticated":false,"identity":"staging","reconnect":"gcloud"}]"#
        let c = client(response: failed)
        let model = SessionModel(client: c)
        for (response, expected) in [(failed, 1), (failed.replacingOccurrences(of: "false", with: "true"), 0)] {
            JourneyTransport.lock.withLock { JourneyTransport.response = response }
            let page = SettingsPage(model: model, pairing: PairingModel(client: c), onClose: {})
            let host = NSHostingView(rootView: page.frame(width: 1000, height: 800))
            host.frame = NSRect(x: 0, y: 0, width: 1000, height: 800)
            host.layoutSubtreeIfNeeded()
            for _ in 0..<100 {
                if !model.tools.isEmpty && page.needsAttention == expected { break }
                try? await Task.sleep(for: .milliseconds(20))
            }
            XCTAssertEqual(model.tools.count, 1)
            XCTAssertEqual(page.needsAttention, expected)
            withExtendedLifetime(host) {}
        }
    }

    func testSetupStreamsNeedSuccessfulExitAndPreserveFatalErrors() {
        var result = SetupCommandResult()
        result.receive(.init(name: "line", data: "working"))
        XCTAssertFalse(result.succeeded)
        XCTAssertTrue(result.failure?.contains("disconnected") == true)
        result.receive(.init(name: "done", data: "1"))
        XCTAssertFalse(result.succeeded)
        XCTAssertTrue(result.failure?.contains("code 1") == true)
        result = SetupCommandResult()
        result.receive(.init(name: "fatal", data: "no credentials"))
        result.receive(.init(name: "done", data: "0"))
        XCTAssertFalse(result.succeeded)
        XCTAssertEqual(result.failure, "no credentials")
        result = SetupCommandResult()
        result.receive(.init(name: "done", data: "0"))
        XCTAssertTrue(result.succeeded)
        XCTAssertNil(result.failure)
    }

    func testCredentialDisconnectUsesAPIIdentifierNotDisplayLabel() {
        XCTAssertEqual(SetupCommandResult.credentialProvider(for: "/api/connect/github"), "github")
        XCTAssertEqual(SetupCommandResult.credentialProvider(for: "/api/connect/cloudflare"), "cloudflare")
    }

    func testGitHubCLIConnectionIsNotAManagedToken() throws {
        let decoder = JSONDecoder()
        let cli = try decoder.decode(ConnectionsSettings.Stored.self, from: Data(#"{"github":{"login":"dev"},"github_via_gh":true,"github_stored":false}"#.utf8))
        XCTAssertFalse(cli.hasGitHubToken)
        let revoked = try decoder.decode(ConnectionsSettings.Stored.self, from: Data(#"{"github":null,"github_stored":true}"#.utf8))
        XCTAssertTrue(revoked.hasGitHubToken, "A revoked stored token still needs Replace/Disconnect controls")
        let legacy = try decoder.decode(ConnectionsSettings.Stored.self, from: Data(#"{"github":{"login":"dev"},"github_via_gh":true}"#.utf8))
        XCTAssertFalse(legacy.hasGitHubToken)
    }

    func testAWSConfigurationTargetsTheSelectedProfileAndQuotesIt() {
        XCTAssertEqual(SetupCommandResult.awsConfigureCommand(profile: ""), "aws configure")
        XCTAssertEqual(SetupCommandResult.awsConfigureCommand(profile: "production"), "aws configure --profile 'production'")
        XCTAssertEqual(SetupCommandResult.awsConfigureCommand(profile: "team's; echo unsafe"), "aws configure --profile 'team'\\''s; echo unsafe'")
    }

    func testSkillCatalogShowsAvailablePluginsWithoutRecommendations() {
        let entry = SkillCatalog.Entry(name: "review", description: "Code review", marketplace: "official", installed: false)
        XCTAssertEqual(SkillCatalog.results(suggested: [], all: [entry], query: "").map(\.name), ["review"])
        XCTAssertEqual(SkillCatalog.results(suggested: [], all: [entry], query: "  CODE  ").count, 1)
        XCTAssertTrue(SkillCatalog.results(suggested: [], all: [entry], query: "missing").isEmpty)
    }

    func testReviewHooksOpensHooksInsteadOfTogglingAnArbitraryPanel() throws {
        let model = SessionModel(client: Client(port: 0))
        model.workspace.hooks = [try JSONDecoder().decode(Wire.Hook.self, from: Data(#"{"event":"Stop","command":"make check","scope":"project","source":".claude/settings.json"}"#.utf8))]
        let received = expectation(description: "Open hooks panel")
        let observer = NotificationCenter.default.addObserver(forName: .keelShowPanel, object: nil, queue: .main) { note in
            XCTAssertEqual(note.object as? String, "hooks")
            received.fulfill()
        }
        defer { NotificationCenter.default.removeObserver(observer) }
        let item = try XCTUnwrap(SetupSheet.items(for: model, settings: {}).first { $0.id == "hooks" })
        item.run()
        wait(for: [received], timeout: 1)
    }

    func testContextPickerButtonsActuallySelectWithoutApplying() throws {
        let c = Client(port: 0)
        let snapshot = try JSONDecoder().decode(KubernetesSetup.Snapshot.self, from: Data(contexts.utf8))
        let setup = KubernetesSetup(client: c, snapshot: snapshot)
        let host = NSHostingView(rootView: KubernetesContextSheet(client: c, setup: setup) {})
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 600, height: 600),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.contentView = host
        window.orderBack(nil)
        defer { window.orderOut(nil); window.contentView = nil }
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        host.layoutSubtreeIfNeeded()
        func scroll(in view: NSView) -> NSScrollView? {
            if let found = view as? NSScrollView { return found }
            return view.subviews.lazy.compactMap { scroll(in: $0) }.first
        }
        let list = try XCTUnwrap(scroll(in: host))
        let document = try XCTUnwrap(list.documentView)
        // Second row, in the actual native scroll document. Send only to this test window.
        let point = document.convert(NSPoint(x: 100, y: document.isFlipped ? 80 : document.bounds.height - 80), to: nil)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            let event = try XCTUnwrap(NSEvent.mouseEvent(with: type, location: point, modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
            window.sendEvent(event)
        }
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        XCTAssertEqual(setup.selected, "staging")
        XCTAssertEqual(setup.current, "dev", "Selecting must not change kubectl until Apply is clicked")
        XCTAssertTrue(setup.canApply)
    }
    func testCaptureToolSetupWhenRequested() throws {
        guard let raw = ProcessInfo.processInfo.environment["KEEL_JOURNEY_CATALOG"] else {
            throw XCTSkip("Set KEEL_JOURNEY_CATALOG for setup screenshots")
        }
        let tools = [ConnectionsSettings.Tool(id: "kubectl", label: "Kubernetes", installed: true,
            authenticated: false, identity: "gke_example_us-central1_staging",
            blocked: "Google Cloud reconnection needed. Renew your Google Cloud sign-in, then Test connection. Your Kubernetes context has not changed.\ngcloud: invalid_grant: Token has been expired or revoked.", reconnect: "gcloud")]
        try capture(ScrollView { ConnectionsSettings(client: Client(port: 0), tools: tools,
            claudeStatus: .init(installed: true, authenticated: true, brew: true))
            .padding(24) }, name: "tools-kubernetes", directory: raw, width: 760, height: 850)

        let c = Client(port: 0)
        let snapshot = try JSONDecoder().decode(KubernetesSetup.Snapshot.self, from: Data(contexts.utf8))
        let setup = KubernetesSetup(client: c, snapshot: snapshot)
        setup.selected = "staging"
        try capture(KubernetesContextSheet(client: c, setup: setup) {}, name: "kubernetes-contexts",
                    directory: raw, width: 600, height: 560)
        let empty = KubernetesSetup(client: c, snapshot: .init(current: nil, contexts: []))
        try capture(KubernetesContextSheet(client: c, setup: empty) {}, name: "kubernetes-empty",
                    directory: raw, width: 600, height: 400)
        setup.failure = "Could not change context. The kubeconfig is read-only. Check its permissions and retry."
        try capture(KubernetesContextSheet(client: c, setup: setup) {}, name: "kubernetes-error",
                    directory: raw, width: 600, height: 620)
        let model = SessionModel(client: c)
        try capture(FileSurface(model: model, path: "example.swift", data: Data("// A native, selectable source viewer\nimport Foundation\n\nlet title = \"Keel\"\nprint(title)\n".utf8)), name: "source-viewer", directory: raw, width: 800, height: 500)
        try capture(SetupSheet(model: model) {}, name: "project-setup", directory: raw, width: 600, height: 700)
        let aws = ConnectionsSettings.Tool(id: "aws", label: "AWS", installed: true,
            authenticated: false, blocked: "The SSO session expired. Choose your profile and sign in again.",
            profiles: ["development", "production"])
        try capture(ScrollView { ConnectionsSettings(client: c, tools: [aws], claudeStatus: .init(installed: true, authenticated: true, brew: true)).padding(24) }, name: "tools-aws", directory: raw, width: 760, height: 1000)
        try capture(SkillCatalog(client: c) {}, name: "skill-catalog", directory: raw, width: 600, height: 480)
        let entry = SkillCatalog.Entry(name: "code-review", description: "Review code before you merge", marketplace: "official", installed: false)
        try capture(SkillCatalog(client: c, catalog: .init(suggested: [], all: [entry])) {}, name: "skill-catalog-available", directory: raw, width: 600, height: 480)
        try capture(SkillCatalog(client: c, catalog: .init(suggested: [], all: [])) {}, name: "skill-catalog-empty", directory: raw, width: 600, height: 480)
        try capture(SkillCatalog(client: c, failure: "The daemon could not be reached. Check Keel is running and retry.") {}, name: "skill-catalog-error", directory: raw, width: 600, height: 480)
        try capture(AddMCP(client: c) {}, name: "add-mcp", directory: raw, width: 480, height: 550)
        try capture(NewSubagent(client: c) {}, name: "new-subagent", directory: raw, width: 600, height: 700)
        try capture(StartProject(client: c, start: .clone) { _, _, _ in }, name: "clone-project", directory: raw, width: 680, height: 760)
        try capture(NewFeature(model: model, lanes: Lanes(client: c, port: 0)) {}, name: "new-feature", directory: raw, width: 700, height: 760)
        try capture(PairingSettings(model: PairingModel(client: c)).padding(24), name: "settings-devices", directory: raw, width: 700, height: 650)
    }

    private func capture<V: View>(_ view: V, name: String, directory: String,
                                  width: CGFloat, height: CGFloat) throws {
        try FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
        let host = NSHostingView(rootView: view
            .frame(width: width, height: height).background(K.C.bg))
        host.frame = NSRect(x: 0, y: 0, width: width, height: height)
        host.layoutSubtreeIfNeeded()
        let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: rep)
        let png = try XCTUnwrap(rep.representation(using: .png, properties: [:]))
        try png.write(to: URL(fileURLWithPath: directory).appendingPathComponent(name + ".png"))
    }
}
