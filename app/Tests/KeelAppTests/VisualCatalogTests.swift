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

        // A turn in flight, because "is it working" is the question the composer has to answer
        // and the still screens never showed it.
        let live = self.model()
        live.running = true
        live.lastEventAt = Date()
        live.record(Data(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"c2","name":"Bash","input":{"command":"grep -oE \"/[a-zA-Z0-9/_:{}-]+\" src/**/*.ts | sort -u | head","description":"Find every route"}}]}}"#.utf8),
                    into: live.turns[0])
        try capture(ChatRail(model: live), named: "conversation-working", in: directory)

        // `/` — the vocabulary the terminal has and Keel dropped on the floor.
        let slash = self.model()
        slash.slashCommands = ["compact", "context", "review", "cost", "code-review:code-review",
                               "ponytail:ponytail", "swiftui-pro", "release"]
        slash.prompt = "/c"
        try capture(ChatRail(model: slash), named: "conversation-slash", in: directory)

        // The turn stopped and is waiting on somebody — the state the whole hook exists for, and
        // the one the catalog had no picture of.
        let asking = self.model()
        asking.running = true
        asking.lastEventAt = Date()
        asking.pending = [decode(Wire.Pending.self, """
            {"id":"p1","tool":"Bash","command":"docker compose up -d --build",
             "rules":["Bash(docker *)"],"session_id":"S"}
            """)]
        try capture(ChatRail(model: asking), named: "conversation-approval", in: directory)

        let questioning = self.model()
        questioning.running = true
        questioning.lastEventAt = Date()
        questioning.pending = [decode(Wire.Pending.self, """
            {"id":"q1","tool":"AskUserQuestion","command":"","rules":[],"session_id":"S",
             "input":{"questions":[{"question":"Which store should the retry counter live in?",
             "header":"Storage","multiSelect":false,
             "options":[{"label":"Redis","description":"Shared across instances, already deployed"},
                        {"label":"Postgres","description":"Durable, one more table"},
                        {"label":"In memory","description":"Fastest, lost on restart"}]}]}}
            """)]
        try capture(ChatRail(model: questioning), named: "conversation-question", in: directory)

        // The status bar is looked at more often than any pane in here and was the one surface the
        // catalogue never showed — which is how a control in it stayed indistinguishable from the
        // readouts beside it for as long as it did.
        let trusted = self.model()
        trusted.trusted = true
        try capture(StatusBar(model: trusted, terminalOpen: .constant(true)) {},
                    named: "status-bar", in: directory, size: CGSize(width: 900, height: 44))

        // The first screen anybody sees, at a real window's width — it was laid out as though the
        // window were as narrow as a panel.
        try capture(Welcome(model: model) {}, named: "welcome", in: directory,
                    size: CGSize(width: 1100, height: 860))

        try capture(ReviewPacketView(model: model), named: "review", in: directory)
        try capture(ChatRail(model: model), named: "conversation", in: directory)
        try capture(TurnStage(model: model), named: "trace", in: directory)
        for panel in SessionWindow.Panel.allCases {
            try capture(SidePanel(panel: panel, model: model) {}, named: "panel-\(panel.rawValue)",
                        in: directory, size: CGSize(width: 320, height: 760))
        }

        // Settings is five panes behind one nav, and it was the last place in the app where two
        // of them were built one way and three another.
        let pairing = PairingModel(client: model.client)
        try capture(SettingsPage(model: model, pairing: pairing) {},
                    named: "settings", in: directory, size: CGSize(width: 900, height: 700))
        try capture(pane { PermissionsSettings(client: model.client, sessionId: nil) },
                    named: "settings-permissions", in: directory)
        try capture(pane { ConnectionsSettings(client: model.client) },
                    named: "settings-tools", in: directory)
        try capture(pane { PrivacySettings() }, named: "settings-privacy", in: directory)
        try capture(pane { AppearanceSettings() }, named: "settings-appearance", in: directory)
    }

    // MARK: - README media

    static let ask = "Make checkout retries safe and observable"
    static let fps = 10

    /// The turn the README's artwork is made of, as the steps it actually arrives in.
    ///
    /// One list, replayed two ways: a still applies all of it and photographs the end, the reel
    /// applies one step at a time and photographs each. Kept as one list on purpose — two fixtures
    /// drift, and the day they do is the day half the README is a picture of an app that no longer
    /// exists.
    ///
    /// `seconds` is per step because the beats are not equally worth reading: the gate landing
    /// earns two and a half, a tool call opening earns one.
    static let script: [(seconds: Double, apply: (SessionModel, Turn) -> Void)] = [
        // Sent, and nothing back yet — the state the composer has to answer "is it working?" in,
        // and the one a still of a finished turn never shows.
        (0.7, { m, _ in m.running = true; m.lastEventAt = Date() }),

        (0.7, { m, t in
            m.record(event(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"Sources/Checkout/RetryPolicy.swift"}}]}}"#), into: t)
        }),
        (0.6, { m, t in
            m.record(event(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"r1","content":"struct RetryPolicy {\n    let attempts = 3\n}"}]}}"#), into: t)
        }),

        // The first write: one file changed, and the pane that says so appears.
        (0.7, { m, t in
            m.record(event(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"Sources/Checkout/RetryPolicy.swift"}}]}}"#), into: t)
            m.record(event(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"e1","content":"Applied 1 edit"}]}}"#), into: t)
        }),
        (0.8, { m, t in
            m.record(event(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"e2","name":"Edit","input":{"file_path":"Sources/Checkout/Telemetry.swift"}}]}}"#), into: t)
            m.record(event(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"e2","content":"Applied 2 edits"}]}}"#), into: t)
        }),
        (0.8, { m, t in
            m.record(event(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"w1","name":"Write","input":{"file_path":"Tests/Checkout/RetryPolicyTests.swift"}}]}}"#), into: t)
            m.record(event(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"w1","content":"Wrote 64 lines"}]}}"#), into: t)
        }),

        // A command, with what it printed. The half a diff does not show.
        (0.8, { m, t in
            m.record(event(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"c1","name":"Bash","input":{"command":"swift test --filter RetryPolicyTests","description":"Run the new tests"}}]}}"#), into: t)
        }),
        (0.9, { m, t in
            m.record(event(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"c1","content":"Executed 6 tests, with 0 failures (0 unexpected) in 0.418 seconds"}]}}"#), into: t)
            m.record(event(#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Retries are now bounded and observable.\n\n### What changed\n- Stop after three attempts with jittered backoff.\n- Record each attempt and the final outcome.\n- Cover exhausted retries and successful recovery.\n\n### Verification\nThe six retry-policy tests passed. The repository-wide check is next; its result appears beside this conversation."}}}"#), into: t)
        }),

        // The project's own gate, running — not the agent's opinion of its own work.
        (0.8, { _, t in t.gate = .running("make check") }),

        // The verdict, and with it the footer: what it cost, how long, how much was cache.
        (1.8, { m, t in
            t.gate = .passed("make check", 11.4)
            m.record(event(#"{"type":"result","total_cost_usd":0.1832,"duration_ms":48210,"usage":{"input_tokens":4120,"output_tokens":2860,"cache_read_input_tokens":61440,"cache_creation_input_tokens":8192}}"#), into: t)
            t.commit = "a1b2c3d"
            t.finished = true
            m.running = false
        }),
    ]

    /// A command Keel will not run until somebody says so.
    ///
    /// The reel this drives is the one that answers "what happens when it wants to do something
    /// you did not ask for" — the question every reader of an agent README arrives with, and the
    /// one a picture of a passing gate does not touch.
    static let approval: [(seconds: Double, apply: (SessionModel, Turn) -> Void)] = [
        (1.4, { m, t in
            m.running = true
            m.lastEventAt = Date()
            m.record(event(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"d0","name":"Read","input":{"file_path":"docker-compose.yml"}}]}}"#), into: t)
        }),

        // The turn stops here. It does not carry on without the command and tell you afterwards.
        (4.6, { m, _ in
            m.pending = [VisualCatalogTests.pending]
        }),

        (2.4, { m, t in
            m.pending = []
            m.record(event(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"d1","name":"Bash","input":{"command":"docker compose up -d --build","description":"Bring the stack up"}}]}}"#), into: t)
            m.record(event(#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"d1","content":"Container checkout-api-1  Started"}]}}"#), into: t)
        }),

        (1.6, { m, t in
            t.gate = .passed("make check", 11.4)
            t.finished = true
            m.running = false
        }),
    ]

    private static let pending = try! JSONDecoder().decode(Wire.Pending.self, from: Data("""
        {"id":"p1","tool":"Bash","command":"docker compose up -d --build",
         "rules":["Bash(docker *)"],"session_id":"S"}
        """.utf8))

    private static func event(_ json: String) -> Data { Data(json.utf8) }

    /// The whole script applied at once: what a still of a finished turn shows.
    private func scripted() -> SessionModel {
        let model = self.model()
        let turn = Turn(prompt: Self.ask)
        model.turns = [turn]
        for step in Self.script { step.apply(model, turn) }
        turn.settle()
        return model
    }

    /// The README's stills, and the frames `make media` assembles its reels from.
    ///
    /// Every frame is the real SwiftUI view drawing real model state, so a change to the app shows
    /// up in the artwork the next time this runs — which is the whole reason the reel is rendered
    /// rather than screen-recorded. What it is *not* is a photograph of a live agent: the session
    /// is [`script`], a fixture, and the README says so next to the picture.
    ///
    /// The reel replays that one script a step at a time. The catalog applies all of it and
    /// photographs the end; sharing the list is what stops the two from drifting into pictures of
    /// two different apps.
    func testCaptureReadmeMediaWhenRequested() throws {
        guard let raw = ProcessInfo.processInfo.environment["KEEL_README_MEDIA"] else {
            throw XCTSkip("Set KEEL_README_MEDIA to render the README artwork")
        }
        let root = URL(fileURLWithPath: raw, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let app = NSApplication.shared
        let oldAppearance = app.appearance
        app.appearance = NSAppearance(named: .darkAqua)
        defer { app.appearance = oldAppearance }

        // The turn: sized to its content rather than to a square, because the pane lays out from
        // the top and the leftover was coming out as half a frame of empty background.
        try capture(TurnStage(model: scripted()), named: "turn", in: root,
                    size: CGSize(width: 900, height: 360))
        try capture(ChatRail(model: waiting()), named: "approval", in: root, size: Self.chat)

        // The turn reel is `TurnStage`, not the conversation: the conversation shows the ask and
        // one line for whatever is running, and everything the reel is *for* — the files as they
        // are touched, the commands with their output, the gate arriving — is in the turn pane.
        // The first cut of this reeled the chat and was ten seconds of an empty rectangle.
        try reel("turn", into: root, size: Self.stage) { TurnStage(model: $0) }
        try reel("approve", into: root, size: Self.chat) { ChatRail(model: $0) }

        let lanes = mediaLanes()
        let workbench = SessionWindow(lanes: lanes, pairing: PairingModel(client: lanes.client))
        let full = CGSize(width: 1400, height: 790)
        try capture(workbench, named: "workspace", in: root, size: full)
        try capture(workbench.overlay(alignment: .top) {
            Palette(model: lanes.active, open: .constant(true)).padding(.top, 76)
        }, named: "commands", in: root, size: full)
        try reel("workflow", into: root, size: full, model: lanes.active) { _ in workbench }

        let setup = ClaudeSetup(client: Client(port: 0), status: .init(installed: false,
            authenticated: false, brew: false))
        try capture(VStack(alignment: .leading, spacing: K.S.lg) {
            Text("From download to your first task.").font(K.F.display).foregroundStyle(K.C.text)
            Text("Your Claude account. No terminal setup required.").font(K.F.reading).foregroundStyle(K.C.dim)
            ClaudeSetupCard(setup: setup)
        }.padding(K.S.xl).background(K.C.bg), named: "setup", in: root,
            size: CGSize(width: 800, height: 380))
        let tools = decode([ConnectionsSettings.Tool].self, """
            [{"id":"gh","label":"GitHub CLI","installed":false,"authenticated":false,
              "install_cmd":"brew install gh","manual":"https://cli.github.com"},
             {"id":"wrangler","label":"Wrangler","installed":false,"authenticated":false,
              "install_cmd":"brew install cloudflare-wrangler"},
             {"id":"docker","label":"Docker","installed":false,"authenticated":false,
              "install_cmd":"brew install --cask docker"}]
            """)
        try capture(ScrollView {
            ConnectionsSettings(client: Client(port: 0), tools: tools,
                claudeStatus: .init(installed: true, version: "Claude Code", authenticated: false, brew: true))
                .padding(K.S.xl)
        }, named: "installers", in: root, size: CGSize(width: 900, height: 800))
    }

    /// A complete workspace, including concurrent lane states. All names and activity are fixtures.
    private func mediaLanes() -> Lanes {
        let lanes = Lanes(client: Client(port: 0), port: 0, remembers: false)
        let model = lanes.active
        model.loaded = true
        model.projectOpenKnown = true
        model.repoPath = "/Users/developer/Projects/payments-platform"
        model.workspace = decode(Wire.Workspace.self, workspaceJSON)
        model.sessions = model.workspace.sessions
        model.title = "Harden checkout retries"
        model.sessionId = "harden-checkout-retries"
        model.isolated = true
        model.worktree = "checkout-retries"
        model.branch = "keel/checkout-retries"
        model.baseBranch = "main"
        model.gateCommand = "make check"
        model.files = ["Sources/Checkout/RetryPolicy.swift", "Sources/Checkout/Telemetry.swift",
                       "Tests/Checkout/RetryPolicyTests.swift", "Package.swift", "Makefile"]
        let turn = Turn(prompt: Self.ask)
        model.turns = [turn]
        for step in Self.script { step.apply(model, turn) }
        turn.settle()

        let second = lanes.newLane(isolated: true)
        second.title = "Trace duplicate webhooks"
        second.sessionId = "duplicate-webhook"
        second.worktree = "webhooks"
        second.turns = [Turn(prompt: "Find why the payment webhook is delivered twice")]
        second.running = true
        second.lastEventAt = Date()
        let third = lanes.newLane(isolated: true)
        third.title = "Plan team permissions"
        third.sessionId = "team-permissions"
        third.mode = "plan"
        third.turns = [Turn(prompt: "Plan roles and permissions for teams")]
        third.running = true
        third.pending = [Self.pending]
        lanes.activeID = model.id
        return lanes
    }

    static let stage = CGSize(width: 900, height: 420)
    static let chat = CGSize(width: 900, height: 510)

    /// The turn stopped, holding, waiting on a person — the state the whole hook exists for.
    private func waiting() -> SessionModel {
        let model = scripted()
        model.running = true
        model.lastEventAt = Date()
        model.pending = [decode(Wire.Pending.self, """
            {"id":"p1","tool":"Bash","command":"docker compose up -d --build",
             "rules":["Bash(docker *)"],"session_id":"S"}
            """)]
        return model
    }

    /// One reel's frames, numbered for `ffmpeg`, in `frames/<name>/`.
    ///
    /// A held state is rendered once and copied: at ten frames a second most of a reel is the same
    /// picture again, and laying it out ninety more times would cost seconds a beat for files that
    /// come out byte-identical anyway.
    private func reel<V: View>(_ name: String, into root: URL, size: CGSize, model seed: SessionModel? = nil,
                               of view: @escaping (SessionModel) -> V) throws {
        let directory = root.appendingPathComponent("frames/\(name)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        let model = seed ?? self.model()
        let turn = Turn(prompt: Self.ask)
        model.turns = [turn]

        var index = 0
        var held = 0.0
        func hold(_ seconds: Double) throws {
            turn.settle()
            let first = String(format: "%03d", index)
            try capture(view(model), named: first, in: directory, size: size)
            let source = directory.appendingPathComponent("\(first).png")
            index += 1
            held += seconds
            for _ in 1 ..< max(1, Int((seconds * Double(Self.fps)).rounded())) {
                try FileManager.default.copyItem(
                    at: source, to: directory.appendingPathComponent(String(format: "%03d.png", index)))
                index += 1
            }
        }

        if name == "turn" || name == "workflow" {
            // The ask, typed. Three glyphs a frame: one at a time reads as a stall, and the beat
            // is there to say a person started this.
            var typed = ""
            for character in Self.ask {
                typed.append(character)
                guard typed.count % 3 == 0 || typed.count == Self.ask.count else { continue }
                model.prompt = typed
                try hold(1 / Double(Self.fps))
            }
            model.prompt = ""
            for step in Self.script {
                step.apply(model, turn)
                try hold(step.seconds)
            }
        } else {
            for step in Self.approval {
                step.apply(model, turn)
                try hold(step.seconds)
            }
        }

        XCTAssertEqual(held, 10, accuracy: 0.05,
                       "the \(name) reel runs \(held)s — the README budgets ten")
    }

    /// A settings pane in the frame the page gives it, so the catalog shows what a reader sees.
    private func pane<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: K.S.lg) { content() }
            .frame(maxWidth: 640, alignment: .leading)
            .padding(K.S.xl)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(K.C.bg)
    }

    private func capture<Content: View>(_ content: Content, named name: String, in directory: URL,
                                        size: CGSize = CGSize(width: 760, height: 760)) throws {
        let view = NSHostingView(rootView: content.frame(width: size.width, height: size.height).background(K.C.bg))
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
