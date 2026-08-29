import AppKit
import Foundation

/// One conversation, and everything on screen because of it.
///
/// One per window. The daemon holds a single open project, but each window drives its own `claude`
/// session — which is only honest because the server now keys approvals and one-time permission
/// rules by session id. Before that, two windows shared one approval queue and one set of
/// "allow once" rules, and a question could surface in the wrong conversation.
@MainActor
@Observable
final class SessionModel: Identifiable {
    let client: Client

    /// Claude Code's own session id. `nil` until the first turn's `system/init` announces one,
    /// which is also why `settings_json` takes an optional session on the Rust side.
    var sessionId: String?

    /// What this lane is called, in the rail.
    ///
    /// Taken from the first thing you ask it, because that is what distinguishes one agent from
    /// another. Every lane used to be called "New session", which made three concurrent agents
    /// three identical rows — the fastest way to make concurrency look pointless.
    var title = "Untitled"

    var turns: [Turn] = []
    var running = false
    var prompt = ""
    /// Prompts typed while the agent works. Queued visibly rather than refused.
    var queued: [String] = []
    var mode = "acceptEdits"

    var branch: String?
    var changes: [Wire.Change] = []
    var pending: [Wire.Pending] = []
    var lastError: String?

    /// The element picked in the preview, waiting for you to say what to do with it.
    var picked: Picked?
    var pickedBefore: NSImage?
    /// Set while a design turn is in flight, so the after-photo knows what to re-photograph.
    var designInFlight: (picked: Picked, before: NSImage?)?
    /// Asks the preview to snapshot the same rect again. Set by the preview pane while it is open.
    var resnapshot: ((Picked) async -> NSImage?)?

    /// The dev server, when there is one.
    var previewURL: String?
    var devDetected: String?
    var devRunning = false
    var picking = false

    /// How wide the previewed page is rendered. Desktop by default — the pane is narrow, and
    /// letting the pane decide meant every site opened in its phone layout.
    var previewWidth: PreviewWidth = .desktop

    /// Take a pick from the preview: stage it, and file its before-image as an attachment so the
    /// agent sees what the thing looks like now.
    func designPick(_ p: Picked, before: NSImage?) {
        picked = p
        pickedBefore = before
        if let before, let tiff = before.tiffRepresentation,
           let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) {
            attach(data: png, name: "before-\(p.tag).png", thumbnail: before, label: "before · \(p.tag)")
        }
    }

    /// Turn the staged pick and what you typed into the prompt that gets sent.
    func designPrompt(_ instruction: String) -> String? {
        guard let picked else { return nil }
        return picked.prompt(instruction: instruction)
    }

    /// Files pasted or dropped into the composer, sent as `@path` mentions.
    var attachments: [Attachment] = []

    /// Review notes, keyed `path:line`.
    ///
    /// Kept on the model rather than in the diff view so they survive the view being rebuilt when
    /// the next turn redraws the stage — losing someone's half-written review because a file list
    /// refreshed is the bug worth designing against.
    var notes: [String: String] = [:]

    /// The notes, as the prompt they become.
    func commentsPrompt() -> String {
        let body = notes.sorted { $0.key < $1.key }
            .map { "\($0.key)\n\($0.value)" }
            .joined(separator: "\n\n")
        return "Review comments on the changes you just made:\n\n" + body
    }

    private var streamTask: Task<Void, Never>?
    private var approvalTask: Task<Void, Never>?

    /// The daemon's port, so views that speak to it directly (the terminal's websocket) do not
    /// each have to be told.
    let port: UInt16

    /// The window this lane belongs to, so a row can open a past session into a new lane rather
    /// than evicting whatever is running here.
    weak var lanes: Lanes?

    /// Identity within the window. Distinct from `sessionId`, which is Claude Code's and does not
    /// exist until the first turn has started.
    let id = UUID()

    /// What this lane is doing, for the rail.
    enum Activity: Equatable {
        case idle
        case working(String)
        case waiting
        case failed
    }

    var activity: Activity {
        if !pending.isEmpty { return .waiting }
        if running {
            let last = current?.calls.last
            return .working(last.map { "\($0.tool) \($0.subject)" } ?? "thinking")
        }
        if case .failed = current?.gate { return .failed }
        return .idle
    }

    /// Take the project-level facts from another lane, so N lanes do not each scan the repository.
    func adopt(project other: SessionModel) {
        branch = other.branch
        isRepo = other.isRepo
        changes = other.changes
        tree = other.tree
        repoPath = other.repoPath
        files = other.files
        sessions = other.sessions
        findings = other.findings
        workspace = other.workspace
        trusted = other.trusted
        gateCommand = other.gateCommand
        previewURL = other.previewURL
        previewWidth = other.previewWidth
        devDetected = other.devDetected
        devRunning = other.devRunning
        missingSuggestions = other.missingSuggestions
        tools = other.tools
        projectOpenKnown = other.projectOpenKnown
    }

    init(client: Client, port: UInt16 = 7777, sessionId: String? = nil) {
        self.client = client
        self.port = port
        self.sessionId = sessionId
    }

    var current: Turn? { turns.last }

    /// The file whose diff is on screen, picked from the changes tree.
    var viewingDiff: String?

    /// What the right pane is inspecting, when it is not showing a turn or the preview.
    ///
    /// Clicking a skill or a hook has to lead somewhere: a row you cannot open is a row that only
    /// tells you a name you already knew.
    var inspecting: Inspect?

    enum Inspect: Equatable {
        case skill(Wire.Named)
        case agent(Wire.Named)
        case mcp(Wire.Named)
        case hook(Wire.Hook)
        case plugin(Wire.Plugin)
    }

    /// Set when a turn is picked in the conversation, so the record beside it scrolls to match.
    /// The two panes scroll independently, which is right — but they have to be able to meet.
    var focusedTurn: UUID?

    /// What this conversation has cost, summed from what the CLI reported per turn. A client-side
    /// estimate, which is what the CLI calls it too.
    var sessionCost: Double? {
        let total = turns.compactMap(\.cost).reduce(0, +)
        return total > 0 ? total : nil
    }

    // MARK: - Sending

    /// Bumped whenever both panes should jump to the end and start following again.
    ///
    /// Sending is one such moment — asking a question means you want to watch the answer. Opening
    /// a past session is the other: the latest exchange is what you came back for, and landing at
    /// the top of a 300-message transcript means scrolling for a while to reach it.
    var pinTick = 0

    func send() {
        let text = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        pinTick += 1
        prompt = ""
        guard !running else { queued.append(text); return }
        start(text)
    }

    private func start(_ text: String) {
        // The first ask names the lane. Later ones do not: renaming a lane mid-conversation would
        // lose the thing you were using to tell it apart.
        if turns.isEmpty {
            let first = text.split(separator: "\n").first.map(String.init) ?? text
            title = String(first.prefix(60))
        }

        // A staged pick turns what you typed into a design prompt, and is handed to the turn so
        // the same element can be photographed again when it finishes.
        let instruction = picked.map { _ in designPrompt(text) ?? text } ?? text
        if let picked { designInFlight = (picked, pickedBefore) }
        self.picked = nil
        pickedBefore = nil

        let full = promptWithAttachments(instruction)
        attachments.removeAll()
        let turn = Turn(prompt: full)
        turns.append(turn)
        running = true
        lastError = nil
        watchApprovals(true)

        var query = ["prompt": full, "mode": mode]
        if let sessionId { query["session"] = sessionId }

        streamTask = Task { [client] in
            do {
                for try await event in client.events("/api/chat", query) {
                    switch event.name {
                    case "msg":
                        if let data = event.data.data(using: .utf8) { self.record(data, into: turn) }
                    case "fatal":
                        self.lastError = event.data
                    case "done":
                        break
                    default:
                        break
                    }
                }
            } catch {
                self.lastError = error.localizedDescription
            }
            await self.endTurn(turn)
        }
    }

    private func endTurn(_ turn: Turn) async {
        turn.finished = true
        running = false
        watchApprovals(false)
        await refreshGit()
        // The agent does not grade its own work.
        if mode != "plan" { await runGate(turn) }
        // After the gate, not before: the notification carries the verdict, and a verdict that
        // arrives before the checks have run is the "done!" that started this whole argument.
        await checkDesign(turn)
        Notifications.turnFinished(files: turn.files.count, gate: turn.gate)
        if !queued.isEmpty { start(queued.removeFirst()) }
    }

    func stop() {
        streamTask?.cancel()
        streamTask = nil
        running = false
        watchApprovals(false)
    }

    // MARK: - The stream
    //
    // `msg` carries a raw Claude Code stream-json line, forwarded verbatim by the daemon — see the
    // comment in api.rs. The daemon translates nothing, so this parser and the web UI's `record()`
    // must agree about the same bytes.

    struct Record: Decodable {
        var type: String
        var subtype: String?
        var session_id: String?
        var model: String?
        var total_cost_usd: Double?
        var duration_ms: Int?
        var message: Message?
        var event: StreamEvent?

        struct Message: Decodable { var content: [Block]? }
        struct Block: Decodable {
            var type: String
            var id: String?
            var name: String?
            var input: [String: JSONValue]?
            var tool_use_id: String?
            var content: JSONValue?
            var is_error: Bool?
        }
        struct StreamEvent: Decodable { var delta: Delta? }
        struct Delta: Decodable {
            var type: String?
            var text: String?
            var thinking: String?
        }
    }

    func record(_ data: Data, into turn: Turn) {
        guard let r = try? JSONDecoder().decode(Record.self, from: data) else { return }

        switch r.type {
        case "system" where r.subtype == "init":
            sessionId = r.session_id ?? sessionId

        case "stream_event":
            guard let d = r.event?.delta else { return }
            if d.type == "text_delta", let t = d.text { turn.text += t }
            if d.type == "thinking_delta", let t = d.thinking { turn.thinking += t }

        case "assistant":
            for b in r.message?.content ?? [] where b.type == "tool_use" {
                guard let id = b.id, let name = b.name else { continue }
                turn.begin(call: id, tool: name, input: b.input ?? [:])
            }

        case "user":
            for b in r.message?.content ?? [] where b.type == "tool_result" {
                guard let id = b.tool_use_id else { continue }
                let output = b.content?.flatText ?? ""
                turn.finish(call: id, output: output, failed: b.is_error ?? false)
                // A dev server the *agent* started announces its URL in its own output, and Keel
                // did not know about it: `/api/dev` only tracks servers Keel launched itself. This
                // is the same trick the web UI used, and it is how "it started the app" becomes
                // "there is something to look at".
                noticeURL(in: output)
            }

        case "result":
            if let c = r.total_cost_usd { turn.cost = c }
            turn.durationMS = r.duration_ms
            sessionId = r.session_id ?? sessionId

        default:
            break
        }
    }

    /// Find a local address in command output and point the preview at it.
    ///
    /// Deliberately narrow: loopback and private addresses only. A tool that prints a docs link
    /// should not repoint the preview at someone's website.
    func noticeURL(in text: String) {
        guard previewURL == nil || !devRunning else { return }
        for match in Self.localURL.matches(in: text, range: NSRange(text.startIndex..., in: text)) {
            guard let range = Range(match.range, in: text) else { continue }
            let url = String(text[range])
            // Ports that are never an app: a database, a mail catcher, Keel's own daemon.
            if url.contains(":7777") { continue }
            previewURL = url
            return
        }
    }

    private static let localURL = try! NSRegularExpression(
        pattern: #"https?://(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\])(?::\d{2,5})?(?:/\S*)?"#)

    // MARK: - The gate

    /// The command the gate will run, before it runs — read from the project rather than invented.
    var gateCommand: String?

    func refreshGatePlan() async {
        if let c: Wire.Check? = try? await client.get("/api/verify/plan") {
            gateCommand = c?.command
        }
    }

    /// Run the gate against the latest turn, on demand.
    func runGateNow() async {
        guard let turn = turns.last else { return }
        await runGate(turn)
    }

    private func runGate(_ turn: Turn) async {
        let started = Date()
        var problems: [Wire.Problem] = []
        var command = ""
        do {
            for try await event in client.events("/api/verify") {
                switch event.name {
                case "none":
                    turn.gate = .none(event.data); return
                case "start":
                    command = event.data
                    turn.gate = .running(command)
                case "problem":
                    if let d = event.data.data(using: .utf8),
                       let p = try? JSONDecoder().decode(Wire.Problem.self, from: d) {
                        problems.append(p)
                    }
                case "done":
                    turn.gate = event.data == "0"
                        ? .passed(command, Date().timeIntervalSince(started))
                        : .failed(command, problems)
                case "fatal":
                    turn.gate = .none(event.data)
                default:
                    break
                }
            }
        } catch {
            turn.gate = .none(error.localizedDescription)
        }
    }

    /// Photograph the picked element again and say what the pixels did.
    ///
    /// Runs after the gate, because a turn that failed its own checks has a more useful thing to
    /// report first — but it runs even then, since "the tests broke and it edited the wrong file"
    /// is two facts, not one.
    private func checkDesign(_ turn: Turn) async {
        guard let flight = designInFlight else { return }
        designInFlight = nil

        // Let the dev server rebuild and the page settle before believing the pixels.
        try? await Task.sleep(for: .milliseconds(900))
        let after = await resnapshot?(flight.picked) ?? nil
        turn.design = Turn.Design(
            selector: flight.picked.selector,
            before: flight.before,
            after: after,
            verdict: DesignCheck.compare(before: flight.before, after: after),
            duplicated: DesignCheck.looksDuplicated(files: turn.files, hints: flight.picked.hints))
    }

    // MARK: - Dev server

    struct DevStatus: Decodable {
        var running: Bool
        var url: String?
        var detected: String?
    }

    func refreshDev() async {
        guard let d: DevStatus = try? await client.get("/api/dev") else { return }
        devRunning = d.running
        devDetected = d.detected
        if let u = d.url { previewURL = u }
    }

    struct StartDev: Encodable { var command: String? }

    func startDev() async {
        struct Ok: Decodable {}
        _ = try? await client.post("/api/dev/start", body: StartDev(command: nil), as: DevStatus.self)
        // The URL appears in the server's own output a moment after it starts.
        for _ in 0..<40 {
            await refreshDev()
            if previewURL != nil { return }
            try? await Task.sleep(for: .milliseconds(400))
        }
    }

    // MARK: - Approvals

    /// Polled only while a turn is running, and only for this conversation.
    private func watchApprovals(_ on: Bool) {
        approvalTask?.cancel()
        approvalTask = nil
        guard on else { pending = []; return }
        approvalTask = Task { [client] in
            while !Task.isCancelled {
                var q: [String: String] = [:]
                if let id = self.sessionId { q["session"] = id }
                if let found: [Wire.Pending] = try? await client.get("/api/approve/poll", q), !found.isEmpty {
                    self.pending.append(contentsOf: found)
                    Notifications.approvalWaiting(found.count)
                }
                try? await Task.sleep(for: .milliseconds(700))
            }
        }
    }

    struct Answer: Encodable {
        var id: String
        var decision: String
        var rules: [String]
        var scope: String
        var session: String?
    }

    func answer(_ p: Wire.Pending, allow: Bool, scope: String) {
        pending.removeAll { $0.id == p.id }
        let body = Answer(id: p.id, decision: allow ? "allow" : "deny",
                          rules: p.rules, scope: scope, session: sessionId)
        Task { [client] in
            struct Ok: Decodable { var ok: Bool }
            _ = try? await client.post("/api/approve/answer", body: body, as: Ok.self)
            // Trusting from an approval changes the window's own claim about itself.
            if scope == "trust" { await self.refreshTrust() }
        }
    }

    // MARK: - Repository

    /// Sessions in this project, newest first. Titles, counts and timestamps only — the daemon
    /// deliberately never returns message bodies to a listing, and a native client is not a reason
    /// to change that.
    var sessions: [Wire.Session] = []
    var findings: [Wire.Finding] = []

    /// Plugins the scanner recommends for this repository that are not installed.
    ///
    /// Surfaced as a badge rather than left in a panel nobody opens: a recommendation you never
    /// see is a recommendation that does nothing.
    var missingSuggestions: [SkillCatalog.Entry] = []

    struct Catalog: Decodable { var suggested: [SkillCatalog.Entry] }

    /// The CLIs Keel drives, and whether each one actually works.
    var tools: [ConnectionsSettings.Tool] = []

    func refreshTools() async {
        tools = (try? await client.get("/api/cli")) ?? []
    }

    func refreshSuggestions() async {
        guard let c: Catalog = try? await client.get("/api/plugins", as: Catalog.self) else { return }
        missingSuggestions = c.suggested.filter { !$0.installed }
    }

    var tree: [Wire.Node] = []
    /// Every path, flat — for the filter and the mention picker.
    var files: [String] = []

    func refreshTree() async {
        guard let t: [Wire.Node] = try? await client.get("/api/tree") else { return }
        tree = t
        var flat: [String] = []
        func walk(_ nodes: [Wire.Node]) {
            for n in nodes {
                if n.dir { walk(n.children ?? []) } else { flat.append(n.path) }
            }
        }
        walk(t)
        files = flat
    }

    /// Attach a file from the tree as an `@path` mention.
    ///
    /// Added to the attachment strip rather than typed into the box: it is context riding along
    /// with the prompt, not part of the sentence, and it should be as removable as a paste.
    func mention(_ path: String) {
        guard !attachments.contains(where: { $0.path == path }) else { return }
        attachments.append(Attachment(path: path, label: path, thumbnail: nil))
    }

    var workspace = Wire.Workspace(sessions: [])
    /// Whether a project is open. `nil` until the daemon has actually answered — which is not the
    /// same as "no project", and treating them the same is what let a failed first request read as
    /// a real answer.
    var projectOpenKnown: Bool?
    var projectOpen: Bool { projectOpenKnown ?? true }

    struct OpenBody: Encodable { var path: String }
    struct Opened: Decodable { var path: String }

    /// Point the daemon at a different repository.
    ///
    /// One project across every window, which is the daemon's own shape — `AppState` holds a
    /// single repo. Switching it moves every window, so this reloads the shared state rather than
    /// pretending the other windows are unaffected.
    func openProject(_ path: String) async throws {
        _ = try await client.post("/api/open", body: OpenBody(path: path), as: Opened.self)
        projectOpenKnown = true
        turns.removeAll()
        sessionId = nil
        title = "New session"
        await refreshGit()
        await refreshState()
        await refreshTree()
        await refreshDev()
    }

    /// The open repository's path, for the project menu and for the Finder.
    var repoPath = ""

    func refreshState() async {
        guard let s: Wire.State = try? await client.get("/api/state") else { return }
        projectOpenKnown = s.projectOpen
        repoPath = s.repo
        // A project resumed from prefs was never seen by this app's own recents list, so the
        // switcher would be empty on a fresh install until you opened something a second time.
        if s.projectOpen, !s.repo.isEmpty { Recents.remember(s.repo) }
        sessions = s.workspace.sessions
        findings = s.scan.findings
        workspace = s.workspace
    }

    /// Open an existing conversation in this window: replay what it did, then carry on.
    func open(session id: String) async {
        sessionId = id
        title = sessions.first { $0.id == id }?.title ?? "Session " + id.prefix(8)
        turns.removeAll()
        // What the session actually said, and what it did to the repository — two different
        // endpoints, because the daemon deliberately keeps the listing away from the bodies.
        async let bodies: [Wire.Turn]? = try? client.get("/api/session", ["id": id])
        async let work: SessionWork? = try? client.get("/api/session/work", ["id": id])
        let (said, did) = await (bodies, work)

        // The conversation, so a replayed session reads as one rather than as a command log. The
        // work is attached to the last turn, which is where a reader looks for "and then what".
        var replayed: [Turn] = []
        var pendingText = ""
        for t in said ?? [] {
            if t.role == "user" {
                let turn = Turn(prompt: t.text)
                turn.finished = true
                replayed.append(turn)
            } else {
                pendingText = t.text
                replayed.last?.text = pendingText
            }
        }
        if replayed.isEmpty { replayed = [Turn(prompt: title)] }

        if let did, let last = replayed.last {
            for f in did.files { last.noteEdit(f) }
            for (i, c) in did.calls.enumerated() {
                last.begin(call: "replay-\(i)", tool: c.tool,
                           input: ["command": .string(c.subject)])
                last.finish(call: "replay-\(i)", output: c.output, failed: c.error)
            }
            last.truncated = did.truncated
        }
        replayed.forEach { $0.finished = true; $0.replayed = true }
        turns = replayed
        pinTick += 1
    }

    struct SessionWork: Decodable {
        var files: [String]
        var calls: [Call]
        var truncated: Bool
        struct Call: Decodable {
            var tool: String
            var subject: String
            var output: String
            var error: Bool
        }
    }

    /// Whether this project is trusted, which the window says out loud for as long as it is true.
    ///
    /// A permission granted once and then forgotten is the one that surprises you later, so it is
    /// not enough for this to be correct — it has to be visible.
    var trusted = false

    struct PermissionsView: Decodable { var trusted: Bool }

    struct PathBody: Encodable { var path: String }
    struct RenameBody: Encodable { var path: String; var name: String }
    struct CreateBody: Encodable { var path: String; var kind: String }
    struct PathReply: Decodable { var path: String }

    /// Show a file in the Finder.
    func reveal(_ path: String) async {
        _ = try? await client.post("/api/fs/reveal", body: PathBody(path: path), as: PathReply.self)
    }

    /// Delete a file. The daemon moves it to the Trash rather than unlinking it, so this is
    /// recoverable — which is the only reason it is offered without a confirmation.
    func delete(_ path: String) async {
        _ = try? await client.post("/api/fs/delete", body: PathBody(path: path), as: PathReply.self)
        await refreshTree()
        await refreshGit()
    }

    func rename(_ path: String, to name: String) async {
        _ = try? await client.post("/api/fs/rename", body: RenameBody(path: path, name: name),
                                   as: PathReply.self)
        await refreshTree()
    }

    struct SessionRename: Encodable { var id: String; var title: String }

    /// Name a session. Kept in Keel's own store — Claude Code's transcript is not touched to
    /// achieve it.
    func rename(session id: String, to title: String) async {
        struct Ok: Decodable {}
        _ = try? await client.post("/api/session/rename",
                                   body: SessionRename(id: id, title: title), as: Bool.self)
        await refreshState()
    }

    struct GitAct: Encodable { var action: String; var path: String }

    /// Stage, unstage or discard one file.
    ///
    /// `discard` on a tracked file is `git restore`; on an untracked one the daemon moves it to
    /// the Trash rather than unlinking it, so a mistaken discard is recoverable.
    func gitAct(_ action: String, _ path: String) async {
        _ = try? await client.post("/api/git/act", body: GitAct(action: action, path: path),
                                   as: Bool.self)
        await refreshGit()
    }

    func stopDev() async {
        struct Ok: Decodable {}
        _ = try? await client.post("/api/dev/stop", body: [String: String](), as: Bool.self)
        await refreshDev()
    }

    func refreshTrust() async {
        if let v: PermissionsView = try? await client.get("/api/permissions") {
            trusted = v.trusted
        }
    }

    /// Whether the project is under git at all. A new one is not, and that is not an error.
    var isRepo = true

    func refreshGit() async {
        if let s: Wire.GitStatus = try? await client.get("/api/git/status") {
            isRepo = s.isRepo
            branch = s.branch
            changes = s.changes
        }
    }

    struct Nothing: Encodable {}

    func gitInit() async -> String? {
        do {
            _ = try await client.post("/api/git/init", body: Nothing(), as: Bool.self)
            await refreshGit()
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    func diff(_ path: String) async -> Wire.Diff? {
        try? await client.get("/api/git/diff", ["path": path])
    }
}
