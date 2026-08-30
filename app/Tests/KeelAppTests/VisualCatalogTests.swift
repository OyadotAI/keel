import AppKit
import SwiftUI
import XCTest
@testable import KeelApp

@MainActor
final class VisualCatalogTests: XCTestCase {
    private func model() -> SessionModel {
        let model = SessionModel(client: Client(port: 0))
        model.loaded = true
        model.repoPath = "/Users/engineer/Projects/payments-platform"
        model.title = "Harden checkout retries"
        model.branch = "feature/checkout-retries"
        model.worktree = "/Users/engineer/.keel/worktrees/checkout-retries"
        model.changes = [
            Wire.Change(path: "Sources/Checkout/RetryPolicy.swift", status: " M", label: "modified"),
            Wire.Change(path: "Tests/Checkout/RetryPolicyTests.swift", status: "??", label: "untracked"),
        ]
        // Populated, because the empty state is the easy half. Every panel has to look composed
        // with real rows in it, and a catalog that only ever shows "nothing here" cannot tell you
        // whether a list is designed. Decoded rather than constructed: the property wrappers on
        // these types block a memberwise initialiser, and this is the route the daemon's own
        // responses take anyway.
        model.workspace = decode(Wire.Workspace.self, workspaceJSON)
        model.sessions = model.workspace.sessions

        let turn = Turn(prompt: "Make checkout retries safe and observable")
        model.record(Data(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"edit","name":"Edit","input":{"file_path":"Sources/Checkout/RetryPolicy.swift"}},{"type":"tool_use","id":"command","name":"Bash","input":{"command":"make check","description":"Verify the repository gate"}}]}}"#.utf8), into: turn)
        model.record(Data(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"command","content":"All checks passed"}]}}"#.utf8), into: turn)
        turn.gate = .passed("make check", 2)
        turn.finished = true
        turn.text = "Implemented bounded retries with structured telemetry and regression coverage."
        model.turns = [turn]
        return model
    }

    private func decode<T: Decodable>(_ type: T.Type, _ json: String) -> T {
        do {
            return try JSONDecoder().decode(T.self, from: Data(json.utf8))
        } catch {
            fatalError("catalog fixture does not decode: \(error)")
        }
    }

    private let workspaceJSON = """
    {
      "sessions": [
        {"id":"harden-checkout-retries","title":"Harden checkout retries","messages":42,
         "last_active":"2026-08-30T09:14:00Z","scope":"here"},
        {"id":"duplicate-webhook","title":"Trace the duplicate webhook","messages":118,
         "last_active":"2026-08-29T17:02:00Z","scope":"here"},
        {"id":"bump-postgres","title":"Bump Postgres to 17","messages":9,
         "last_active":"2026-08-27T11:40:00Z","scope":"above"}
      ],
      "skills": [
        {"name":"code-review","description":"Review a diff for correctness and scope","scope":"user"},
        {"name":"release-notes","description":"Write the changelog from merged PRs","scope":"user"},
        {"name":"db-migrate","description":"Plan and apply a schema migration","scope":"project"}
      ],
      "agents": [
        {"name":"contract","description":"Reads the code the way a staff engineer would","scope":"user"},
        {"name":"perf","description":"Hot paths, allocations and query plans","scope":"project"}
      ],
      "plugins": [
        {"name":"cloudflare","enabled":true,"marketplace":"anthropics","scope":"user"},
        {"name":"postgres-tools","enabled":false,"marketplace":"community","scope":"project"}
      ],
      "hooks": [
        {"event":"PreToolUse","command":"./scripts/audit.sh","scope":"project",
         "source":".claude/settings.json"},
        {"event":"Stop","command":"make check","scope":"user","source":""}
      ],
      "mcp_servers": [
        {"name":"linear","description":"Issues and cycles","scope":"user"},
        {"name":"sentry","description":"Errors for this service","scope":"project"}
      ]
    }
    """

    func testCaptureVisualCatalogWhenRequested() throws {
        guard let raw = ProcessInfo.processInfo.environment["KEEL_VISUAL_CATALOG"] else {
            throw XCTSkip("Set KEEL_VISUAL_CATALOG to capture the design catalog")
        }
        let directory = URL(fileURLWithPath: raw, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        let model = model()
        let lanes = Lanes(client: Client(port: 0), port: 0)

        try capture(ReviewPacketView(model: model, lanes: lanes), named: "review", in: directory)
        try capture(ChatRail(model: model), named: "conversation", in: directory)
        try capture(TurnStage(model: model), named: "trace", in: directory)
        for panel in SessionWindow.Panel.allCases {
            try capture(SidePanel(panel: panel, model: model), named: "panel-\(panel.rawValue)",
                        in: directory, size: CGSize(width: 320, height: 760))
        }
    }

    private func capture<Content: View>(_ content: Content, named name: String, in directory: URL,
                                        size: CGSize = CGSize(width: 760, height: 760)) throws {
        let view = NSHostingView(rootView: content.frame(width: size.width, height: size.height))
        view.frame = CGRect(origin: .zero, size: size)
        view.layoutSubtreeIfNeeded()
        guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
            return XCTFail("Could not create bitmap for \(name)")
        }
        view.cacheDisplay(in: view.bounds, to: bitmap)
        guard let data = bitmap.representation(using: .png, properties: [:]) else {
            return XCTFail("Could not encode \(name)")
        }
        try data.write(to: directory.appendingPathComponent("\(name).png"), options: .atomic)
    }
}
