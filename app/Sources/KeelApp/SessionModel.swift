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

    /// This lane's own checkout, once it has one. Created on the first send rather than on
    /// `+`, so the branch can be named after what the lane is for.
    var worktree: String?
    /// Whether this lane gets a checkout of its own. Set at creation; a lane that shares the
    /// project's working tree is for reading and planning beside one that is editing.
    var isolated = false
    /// Not a tab: a lane doing work on the person's behalf, surfaced when it has something.
    var hidden = false
    /// Torn out into a window of its own: the main window's tabs leave it alone while it is.
    var detached = false

    /// The query every checkout-scoped request carries, so the daemon reads and writes this
    /// lane's tree rather than the project's.
    func q(_ extra: [String: String] = [:]) -> [String: String] {
        var out = extra
        if let worktree { out["wt"] = worktree }
        return out
    }

    /// Where a resumed session was launched, when that was a folder above or below the repo.
    /// Sent with the chat and transcript requests so `--resume` finds it.
    var sessionCwd: String?
    private func sq(_ extra: [String: String] = [:]) -> [String: String] {
        var out = q(extra)
        if let sessionCwd { out["cwd"] = sessionCwd }
        return out
    }

    /// Bumped when the composer should take focus. The shortcut is handled by the window, which
    /// stays mounted; the text view is the only thing that can actually focus itself.
    var focusComposerTick = 0
    /// Whether the daemon has answered `/api/state` at least once. Before that, every empty
    /// list is "not loaded yet", not "nothing here".
    var loaded = false

    var turns: [Turn] = []
    var running = false
    /// When the stream last said anything; the working bar reads silence off it.
    var lastEventAt = Date()
    /// One stall report per turn.
    var stallReported = false
    var prompt = ""
    /// Prompts typed while the agent works. Queued visibly rather than refused.
    var queued: [String] = []
    var mode = "acceptEdits"
    /// `--model` for every turn, or empty for the CLI's default. The same choice `/model` makes
    /// in the terminal.
    var claudeModel: String {
        get { UserDefaults.standard.string(forKey: "keel.model") ?? "" }
        set { UserDefaults.standard.set(newValue, forKey: "keel.model"); modelTick += 1 }
    }
    var modelTick = 0

    var branch: String?
    var changes: [Wire.Change] = []

    /// The files the agent wrote in this conversation, newest turn last, as the Changes panel
    /// shows them — what happened here, not git's view of the working tree (that is the Git tab).
    var editedThisSession: [Wire.Change] {
        var seen: [String: Wire.Change] = [:]
        var order: [String] = []
        for turn in turns {
            for call in turn.calls where Turn.writeTools.contains(call.tool) {
                var path = call.subject
                if !repoPath.isEmpty, path.hasPrefix(repoPath + "/") { path = String(path.dropFirst(repoPath.count + 1)) }
                if let wt = worktree, path.contains("/.keel/worktrees/\(wt)/") {
                    path = String(path.split(separator: "/.keel/worktrees/\(wt)/", maxSplits: 1).last ?? Substring(path))
                }
                guard !path.isEmpty, !path.hasPrefix("/") else { continue }
                let status = call.tool == "Write" && seen[path] == nil ? "A" : "M"
                if seen[path] == nil { order.append(path) }
                seen[path] = Wire.Change(path: path, status: status, label: call.tool == "Write" ? "written" : "edited")
            }
        }
        return order.compactMap { seen[$0] }
    }
    var pending: [Wire.Pending] = []
    var lastError: String?

    /// Notes left on the page, Figma-style: each is an element and what to do about it. They
    /// stack, they stay pinned on the page across hot reloads, and they go with the next send.
    struct Pin: Identifiable {
        let id = UUID()
        var picked: Picked
        var before: NSImage?
        var note = ""
    }
    var pins: [Pin] = []

    /// The pins the turn in flight was sent with, so the after-photos know what to re-shoot.
    var designInFlight: [Pin] = []
    /// Asks the preview to photograph a rect of the page as it is now. Set by the pane while open.
    var resnapshot: ((Picked.Rect) async -> NSImage?)?
    /// Sends a message to the canvas script in the page. Set by the pane while open.
    var canvas: (([String: Any]) -> Void)?

    /// The frontend file the agent is writing right now, for the bar over the preview.
    var editing: String?
    /// Bring the Designer forward and follow the agent to the page it edits.
    var followEdits: Bool {
        // Off by default: the trace is the tab engineers read, and a stage that switches to
        // the page on its own is the thing they turned off first. On is a choice.
        get { UserDefaults.standard.object(forKey: "keel.followEdits") as? Bool ?? false }
        set { UserDefaults.standard.set(newValue, forKey: "keel.followEdits"); designTick += 1 }
    }
    /// Bumped when the window should show the preview because the agent is editing it.
    var designTick = 0
    /// What the last write changed on the page, as the page reported it.
    var changedRegions: [Region] = []
    /// Bumped by every `changed` report; `checkDesign` waits on it instead of on a timer.
    private var changeTick = 0

    /// A frontend file is being written. Show the page it is, and arm the observer.
    private func frontendEdit(_ path: String) {
        editing = path
        if let route = Frontend.route(for: path), let url = previewURL,
           let origin = Frontend.origin(of: url), url != origin + route {
            previewURL = origin + route
        }
        // If a pin's likely source is this file, that pin's element is what is about to move.
        if let pin = pins.first(where: { $0.picked.hints.contains { path.hasSuffix(
            $0.value.split(separator: ":").first.map(String.init) ?? $0.value) } }) {
            canvas?(["keel": "outline", "selector": pin.picked.selector])
        }
        canvas?(["keel": "expect"])
        if followEdits { designTick += 1 }
    }

    /// The page reported what moved.
    func regionsChanged(_ regions: [Region]) {
        changedRegions = regions
        changeTick += 1
        // While a turn runs, the write is the current turn's; after it ended it belongs to the
        // last one. Either way the trace shows the after-image beside the code.
        if let turn = current, let union = Picked.Rect.union(regions.map(\.rect)) {
            Task {
                let shot = await resnapshot?(union) ?? nil
                turn.design = Turn.Design(
                    selector: turn.design?.selector ?? "",
                    before: turn.design?.before,
                    after: turn.design?.after,
                    verdict: turn.design?.verdict ?? .unstable,
                    duplicated: turn.design?.duplicated ?? false,
                    regions: regions, pageAfter: shot)
            }
        }
    }

    func clearRegions() {
        changedRegions = []
        canvas?(["keel": "clear"])
        syncCanvas()
    }

    /// Draw what the model knows onto the page: the pins.
    func syncCanvas() {
        canvas?(["keel": "pins", "pins": pins.map { ["id": $0.id.uuidString, "selector": $0.picked.selector] }])
    }

    func removePin(_ id: UUID) {
        pins.removeAll { $0.id == id }
        syncCanvas()
    }

    /// The dev server, when there is one.
    var previewURL: String?
    var devDetected: String?
    var devRunning = false
    var picking = false

    /// How wide the previewed page is rendered. Desktop by default — the pane is narrow, and
    /// letting the pane decide meant every site opened in its phone layout.
    var previewWidth: PreviewWidth = .desktop

    /// Take a pick from the preview: it becomes a pin, and its before-image rides along as an
    /// attachment so the agent sees what the thing looks like now.
    func designPick(_ p: Picked, before: NSImage?) {
        // Clicking the same element again focuses its pin rather than stacking a second.
        if let i = pins.firstIndex(where: { $0.picked.selector == p.selector }) {
            pins[i].picked = p
            pins[i].before = before ?? pins[i].before
        } else {
            pins.append(Pin(picked: p, before: before))
            Telemetry.track("pin_added", ["hints": p.hints.count])
            if let before, let tiff = before.tiffRepresentation,
               let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) {
                attach(data: png, name: "before-\(p.tag).png", thumbnail: before,
                       label: "pin \(pins.count) · \(p.tag)")
            }
        }
        picking = false
        syncCanvas()
    }

    /// The pins and what you typed, as the prompt that gets sent. One prompt for all of them:
    /// sending notes one at a time makes the agent swing back and forth.
    func designPrompt(_ instruction: String) -> String? {
        guard !pins.isEmpty else { return nil }
        var out = pins.count == 1
            ? "Change this element in the running app:\n\n"
            : "Change these \(pins.count) elements in the running app. Each is pinned on the page:\n\n"
        for (i, pin) in pins.enumerated() {
            if pins.count > 1 { out += "## Pin \(i + 1)\n" }
            out += pin.picked.describe()
            if !pin.note.isEmpty { out += "\n\nNote on this element: \(pin.note)" }
            out += "\n\n"
        }
        return out + instruction
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
            // The deepest running call: while a subagent works, "Task survey the auth flow"
            // for a minute says less than the file it is reading right now.
            let last = current?.calls.last?.deepestRunning ?? current?.calls.last
            return .working(last.map { "\($0.tool) \($0.subject)" } ?? "thinking")
        }
        if case .failed = current?.gate { return .failed }
        return .idle
    }

    /// Take the project-level facts from another lane, so N lanes do not each scan the repository.
    func adopt(project other: SessionModel) {
        // Only what is about the project. A lane with its own checkout has its own branch,
        // changes and tree, and copying the project's over them is how an edit reads as landing
        // in the wrong place.
        if worktree == nil {
            branch = other.branch
            isRepo = other.isRepo
            changes = other.changes
            tree = other.tree
            files = other.files
        }
        repoPath = other.repoPath
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
    /// The file whose contents are on screen, picked from the file tree.
    var viewingFile: String?

    /// One thing on the stage at a time. The stage picks the first of inspector, commit, file,
    /// diff that is set, so setting a diff while a file was open showed the file — and the
    /// click looked dead until the file was closed.
    func show(diff path: String) { viewingDiff = path; viewingFile = nil; viewingCommit = nil; inspecting = nil }
    func show(file path: String) { viewingFile = path; viewingDiff = nil; viewingCommit = nil; inspecting = nil }
    func show(commit c: Wire.Commit) { viewingCommit = c; viewingDiff = nil; viewingFile = nil; inspecting = nil }

    /// A file's bytes, for the viewer. Bounded to the repository by the daemon.
    func raw(_ path: String) async -> Data? {
        try? await client.raw("/api/raw", q(["path": path]))
    }

    /// The one sheet the window can show, presented from the side panel's root rather than
    /// from a row: a `.sheet` on a row inside a lazy stack loses its anchor when the row is
    /// recycled, and a sheet with no anchor is a crash on the way out of it.
    enum Sheet: String, Identifiable {
        case pr, skills, subagent, mcp, setup
        var id: String { rawValue }
    }
    var sheet: Sheet?

    /// Offer the setup checklist once per project, the first time it is opened with something
    /// missing. Reachable any time from ⌘K.
    func offerSetup() {
        guard !repoPath.isEmpty, loaded else { return }
        let key = "keel.setupSeen." + repoPath
        guard !UserDefaults.standard.bool(forKey: key) else { return }
        let needs = !SetupSheet.items(for: self) {}.isEmpty
        UserDefaults.standard.set(true, forKey: key)
        if needs { sheet = .setup }
    }

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

    /// Every token this conversation has spent, summed from what the CLI reported per turn.
    var sessionTokens: Turn.Tokens? {
        let all = turns.compactMap(\.tokens)
        guard !all.isEmpty else { return nil }
        return all.reduce(Turn.Tokens()) { $0 + $1 }
    }

    /// How full the context window is: the last request's input. Tokens, not a percentage —
    /// the window size is not in the stream and a guessed denominator is a lie with a unit.
    var contextTokens: Int? { turns.last(where: { $0.contextTokens > 0 })?.contextTokens }

    /// Dollars per minute, from finished turns. The number people ask for as "how fast is this
    /// burning" — shown while running, when it is the question, and computed from the turns
    /// that have a cost, since the live one does not until it ends.
    var burnRate: Double? {
        let done = turns.filter { $0.cost != nil && ($0.durationMS ?? 0) > 0 }
        let ms = done.reduce(0) { $0 + ($1.durationMS ?? 0) }
        let cost = done.reduce(0.0) { $0 + ($1.cost ?? 0) }
        guard ms > 0, cost > 0 else { return nil }
        return cost / (Double(ms) / 60_000)
    }

    // MARK: - Sending

    /// Bumped whenever both panes should jump to the end and start following again.
    ///
    /// Sending is one such moment — asking a question means you want to watch the answer. Opening
    /// a past session is the other: the latest exchange is what you came back for, and landing at
    /// the top of a 300-message transcript means scrolling for a while to reach it.
    var pinTick = 0

    func send() {
        var text = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        // Pins with notes are a request on their own; the box may stay empty.
        if text.isEmpty, pins.contains(where: { !$0.note.isEmpty }) {
            text = "Apply the notes on the pinned elements."
        }
        guard !text.isEmpty else { return }
        // `# something` is a note for the project, not a turn — the same thing `#` does in the
        // terminal. It goes into CLAUDE.md, which the agent reads every turn.
        if text.hasPrefix("#"), !text.hasPrefix("##") {
            prompt = ""
            Task { await remember(text) }
            return
        }
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

        // Pins turn what you typed into a design prompt, and are handed to the turn so the same
        // elements can be photographed again when it finishes.
        let instruction = designPrompt(text) ?? text
        designInFlight = pins
        pins = []
        changedRegions = []
        canvas?(["keel": "clear"])

        let full = promptWithAttachments(instruction)
        attachments.removeAll()
        Telemetry.track("turn_started", ["mode": mode, "pins": designInFlight.count,
                                         "attachments": attachments.count])
        Telemetry.breadcrumb("turn started")
        let turn = Turn(prompt: full)
        turns.append(turn)
        stallReported = false; lastEventAt = Date(); running = true
        lastError = nil
        watchApprovals(true)

        streamTask = Task { [client] in
            // An isolated lane gets its checkout now, named for what it is about to do. If the
            // repository cannot branch — no commits yet — the lane says so and shares the tree.
            if isolated, worktree == nil {
                await makeWorktree()
            }
            var query = sq(["prompt": full, "mode": mode, "lane": id.uuidString])
            if !claudeModel.isEmpty { query["model"] = claudeModel }
            if let sessionId { query["session"] = sessionId }
            if let system = nextSystem { query["system"] = system; nextSystem = nil }

            // Before the agent touches anything: what the tree looked like, so "restore to
            // before this turn" has something to restore to. A failure here is not a reason to
            // refuse the turn — there is simply no rewind for it, and the menu says so by absence.
            if let snap: Snapshot = try? await client.post("/api/git/snapshot", body: Nothing(),
                                                            q(), as: Snapshot.self) {
                turn.snapshot = snap.tree
            }
            do {
                for try await event in client.events("/api/chat", query) {
                    lastEventAt = Date()
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
        editing = nil
        watchApprovals(false)
        await refreshGit()
        await lanes?.refreshWorktrees()
        // The agent does not grade its own work.
        if mode != "plan" { await runGate(turn) }
        // Accepted work becomes a commit, so the tree stays small and every step is a place
        // to go back to.
        if mode != "plan" { await commitTurn(turn) }
        // After the gate, not before: the notification carries the verdict, and a verdict that
        // arrives before the checks have run is the "done!" that started this whole argument.
        await checkDesign(turn)
        Notifications.turnFinished(lane: self, files: turn.files.count, gate: turn.gate)
        reportTurn(turn)
        if !queued.isEmpty { start(queued.removeFirst()) }
    }

    func stop() {
        streamTask?.cancel()
        streamTask = nil
        running = false
        watchApprovals(false)
    }

    // MARK: - The checkout

    struct WorktreeName: Encodable { var name: String; var from: String? }
    /// The branch this lane was told to start from, when the person chose one.
    var baseBranch: String?

    /// Create this lane's checkout, named from its title.
    func makeWorktree() async {
        let name = Self.slug(title) + "-" + String(UUID().uuidString.prefix(3)).lowercased()
        do {
            let made: Wire.Worktree = try await client.post("/api/worktree/create",
                                                            body: WorktreeName(name: name, from: baseBranch))
            worktree = made.name
            await refreshGit()
            await refreshTree()
            await lanes?.refreshWorktrees()
        } catch {
            isolated = false
            lastError = "This feature shares the project's working tree: " + error.localizedDescription
        }
    }

    /// A branch name from a sentence: `Fix the billing tests` → `fix-the-billing-tests`.
    static func slug(_ text: String) -> String {
        var out = ""
        var dash = true
        for c in text.lowercased().unicodeScalars {
            if c.properties.isASCIIHexDigit || (c.value >= 97 && c.value <= 122) {
                out.unicodeScalars.append(c); dash = false
            } else if !dash {
                out += "-"; dash = true
            }
            if out.count >= 24 { break }
        }
        while out.hasSuffix("-") { out.removeLast() }
        return out.isEmpty ? "lane" : out
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
        /// On a `result`: the whole turn's tokens.
        var usage: Usage?
        /// Set on every record a subagent produced: the `Task` call it is working for.
        var parent_tool_use_id: String?

        struct Message: Decodable {
            var content: [Block]?
            /// On an `assistant` message: what that one request cost, and therefore — input plus
            /// what was read from cache — how full the context window is right now.
            var usage: Usage?
        }
        struct Usage: Decodable {
            var input_tokens: Int?
            var output_tokens: Int?
            var cache_read_input_tokens: Int?
            var cache_creation_input_tokens: Int?
            var context: Int {
                (input_tokens ?? 0) + (cache_read_input_tokens ?? 0) + (cache_creation_input_tokens ?? 0)
            }
        }
        struct Block: Decodable {
            var type: String
            var id: String?
            var name: String?
            var text: String?
            var input: [String: JSONValue]?
            var tool_use_id: String?
            var content: JSONValue?
            var is_error: Bool?
        }
        struct StreamEvent: Decodable {
            var type: String?
            var delta: Delta?
            var content_block: ContentBlock?
        }
        struct ContentBlock: Decodable { var type: String? }
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
            let fresh = sessionId == nil && r.session_id != nil
            sessionId = r.session_id ?? sessionId
            // Claude Code only titles a session when its own UI asks for one, so a session
            // Keel drove would list as a bare id. The lane's title is the first ask; it goes
            // into Keel's own name store, which is what History reads.
            if fresh, let id = sessionId, title != "Untitled" {
                Task { await rename(session: id, to: title) }
            }

        case "stream_event":
            // A new text block after a tool call is a new paragraph. Without this the second
            // message's first word landed flush against the first message's last one.
            if r.event?.type == "content_block_start", r.event?.content_block?.type == "text",
               !turn.text.isEmpty, !turn.text.hasSuffix("\n\n") {
                turn.text += turn.text.hasSuffix("\n") ? "\n" : "\n\n"
            }
            guard let d = r.event?.delta else { return }
            if d.type == "text_delta", let t = d.text { turn.text += t; turn.streamedText = true }
            if d.type == "thinking_delta", let t = d.thinking { turn.thinking += t }

        case "assistant":
            if let u = r.message?.usage, u.context > 0 { turn.contextTokens = u.context }
            // Without partial messages the prose arrives only here, whole. Take it when nothing
            // streamed it first.
            if turn.streamedText == false {
                for b in r.message?.content ?? [] where b.type == "text" {
                    if let t = b.text, !t.isEmpty {
                        if !turn.text.isEmpty { turn.text += "\n\n" }
                        turn.text += t
                    }
                }
            }
            for b in r.message?.content ?? [] where b.type == "tool_use" {
                guard let id = b.id, let name = b.name else { continue }
                turn.begin(call: id, tool: name, input: b.input ?? [:],
                           parent: r.parent_tool_use_id)
                // The moment the agent starts writing a frontend file, the page is the thing to
                // look at — the loop every vibe-coding tool is criticised for not closing.
                if Turn.writeTools.contains(name),
                   let path = b.input?["file_path"]?.stringValue ?? b.input?["path"]?.stringValue,
                   Frontend.isUI(path) {
                    frontendEdit(path)
                }
            }

        case "user":
            for b in r.message?.content ?? [] where b.type == "tool_result" {
                guard let id = b.tool_use_id else { continue }
                let output = b.content?.flatText ?? ""
                turn.finish(call: id, output: output, failed: b.is_error ?? false)
                // The write landed; the dev server is about to rebuild. Arm the page again so a
                // slow HMR is still caught, and keep the bar up until the next call starts.
                if editing != nil { canvas?(["keel": "expect"]) }
                // A dev server the *agent* started announces its URL in its own output, and Keel
                // did not know about it: `/api/dev` only tracks servers Keel launched itself. This
                // is the same trick the web UI used, and it is how "it started the app" becomes
                // "there is something to look at".
                noticeURL(in: output)
            }

        case "result":
            if let c = r.total_cost_usd { turn.cost = c }
            turn.durationMS = r.duration_ms
            if let u = r.usage {
                turn.tokens = Turn.Tokens(
                    input: u.input_tokens ?? 0, output: u.output_tokens ?? 0,
                    cacheRead: u.cache_read_input_tokens ?? 0,
                    cacheWrite: u.cache_creation_input_tokens ?? 0)
            }
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
        if let c: Wire.Check? = try? await client.get("/api/verify/plan", q()) {
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
            for try await event in client.events("/api/verify", q()) {
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

    /// Photograph the pinned elements again and say what the pixels did.
    ///
    /// Runs after the gate, because a turn that failed its own checks has a more useful thing to
    /// report first — but it runs even then, since "the tests broke and it edited the wrong file"
    /// is two facts, not one. Waits for the page to report a change rather than for a timer: the
    /// observer is the "HMR has landed" signal, and a fixed delay was wrong in both directions.
    private func checkDesign(_ turn: Turn) async {
        let flight = designInFlight
        designInFlight = []
        guard let first = flight.first else { return }

        let seen = changeTick
        for _ in 0..<60 where changeTick == seen && !turn.files.isEmpty {
            try? await Task.sleep(for: .milliseconds(100))
        }
        if changeTick == seen { try? await Task.sleep(for: .milliseconds(400)) }

        let after = await resnapshot?(first.picked.rect) ?? nil
        turn.design = Turn.Design(
            selector: first.picked.selector,
            before: first.before,
            after: after,
            verdict: DesignCheck.compare(before: first.before, after: after),
            duplicated: DesignCheck.looksDuplicated(
                files: turn.files, hints: flight.flatMap(\.picked.hints)),
            regions: turn.design?.regions ?? changedRegions,
            pageAfter: turn.design?.pageAfter)
    }

    // MARK: - Dev server

    struct DevStatus: Decodable {
        var running: Bool
        var url: String?
        var detected: String?
        var log: [String]?
    }

    /// The dev server's last lines, for when the page is blank and the reason is in them.
    var devLog: [String] = []
    /// What went wrong loading the page, from the web view itself.
    var previewProblem: String?
    /// Bumped to ask the web view to reload the page it has.
    var reloadTick = 0

    func refreshDev() async {
        guard let d: DevStatus = try? await client.get("/api/dev", q()) else { return }
        devRunning = d.running
        devDetected = d.detected
        devLog = d.log ?? []
        if let u = d.url { previewURL = u }
    }

    struct StartDev: Encodable { var command: String? }

    func startDev() async {
        struct Ok: Decodable {}
        _ = try? await client.post("/api/dev/start", body: StartDev(command: nil), q(), as: DevStatus.self)
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
        approvalTask = Task { [weak self, client] in
            // Weak, so a closed lane stops polling instead of living on inside its own task.
            while !Task.isCancelled {
                guard let self else { return }
                var q: [String: String] = ["lane": self.id.uuidString]
                if let id = self.sessionId { q["session"] = id }
                if let found: [Wire.Pending] = try? await client.get("/api/approve/poll", q), !found.isEmpty {
                    self.pending.append(contentsOf: found)
                    Notifications.approvalWaiting(lane: self, found.count)
                    // So a tester's "it just sat there" can be read against "a question was
                    // shown and never answered": the tool, not the command.
                    for p in found {
                        self.approvalsThisTurn += 1
                        Telemetry.track("approval_shown", ["tool": p.tool, "question": p.isQuestion,
                                                           "rules": p.rules.count, "mode": self.mode])
                        Telemetry.breadcrumb("approval shown: \(p.tool)")
                    }
                }
                // A turn with no output for five minutes is the thing testers describe as
                // "stuck on thinking". Said once per turn, with what it was doing.
                if self.running, !self.stallReported, Date().timeIntervalSince(self.lastEventAt) > 300 {
                    self.stallReported = true
                    let tool = self.current?.calls.last?.tool ?? (self.pending.isEmpty ? "none" : "waiting-on-person")
                    Telemetry.track("turn_stalled", ["tool": tool, "pending": self.pending.count, "mode": self.mode])
                    Telemetry.warn("turn silent for 5 minutes", ["tool": tool, "mode": self.mode, "pending": "\(self.pending.count)"])
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
        var answer: String = ""
    }

    /// Answer a question. The text is what the agent reads as the tool's result.
    func answer(_ p: Wire.Pending, text: String) {
        pending.removeAll { $0.id == p.id }
        Telemetry.track("question_answered")
        let body = Answer(id: p.id, decision: "deny", rules: [], scope: "session",
                          session: sessionId, answer: text)
        Task { [client] in
            struct Ok: Decodable { var ok: Bool }
            _ = try? await client.post("/api/approve/answer", body: body, as: Ok.self)
        }
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
    /// The whole scan: score, profile, plan. `findings` stays the list the badge counts.
    var scan: Wire.Scan?

    struct Adopted: Decodable { var written: [String]; var skipped: [String] }
    struct ReviewPrompt: Decodable { var system: String; var evidence: String; var prompt: String }
    /// System prompt for the next turn only: a persona and its evidence, consumed by `start`.
    var nextSystem: String?

    /// Write the reviewers and the production checklist into the project (never overwriting),
    /// then rescan so the finding clears.
    /// A finding's click is the fix, not a draft of it: the one Keel can write itself is
    /// written; every other goes to the agent now, in a mode that may edit.
    func fix(_ f: Wire.Finding) async {
        if f.id == "agent/no-reviewers" { _ = await adoptPractices(); return }
        mode = "acceptEdits"
        prompt = "Fix this readiness finding: \(f.title)\n\n\(f.detail)"
        send()
    }

    struct IgnoreBody: Encodable { var id: String; var ignored: Bool; var why: String }

    /// Set a finding aside, or bring it back. Written to `.keel/ignored.json` — a decision
    /// about the project, in the project, where the next person can argue with it.
    func ignore(_ id: String, _ ignored: Bool) async {
        await attempt {
            _ = try await client.post("/api/readiness/ignore",
                                      body: IgnoreBody(id: id, ignored: ignored, why: ""),
                                      q(), as: Bool.self)
        }
        await refreshState()
        Telemetry.track(ignored ? "finding_ignored" : "finding_restored", ["id": id])
    }

    func adoptPractices() async -> [String] {
        var written: [String] = []
        await attempt {
            let a: Adopted = try await client.post("/api/adopt", body: Empty(), q(), as: Adopted.self)
            written = a.written
        }
        await refreshState()
        return written
    }

    /// Ask for the staff-engineer review: plan mode, so the turn reads and proposes and
    /// changes nothing; the scan travels as evidence; the persona is the system prompt.
    func requestReview() async {
        // Never in the lane being worked in: a review is a second reader, and it gets its own
        // tab. An empty lane is fine to use — nothing is displaced.
        // Always its own lane, hidden until it has something: a tab that appears on its own
        // and starts talking is what made the review look like an intruder.
        let target: SessionModel = lanes?.reviewLane(beside: self) ?? self
        await target.attempt {
            let r: ReviewPrompt = try await target.client.get("/api/review", target.q())
            target.mode = "plan"
            target.nextSystem = r.system + r.evidence
            target.prompt = r.prompt
            target.send()
            UserDefaults.standard.set(Date(), forKey: reviewKey)
        }
        lastReview = Date()
    }

    /// The lane holding the latest review: this one, or the hidden one beside it.
    var reviewLane: SessionModel? {
        if title == "Staff review" { return self }
        return lanes?.lanes.last(where: { $0.title == "Staff review" })
    }
    /// Bring the review's lane forward as a tab.
    func openReview() {
        guard let lane = reviewLane, let lanes else { return }
        lane.hidden = false
        lanes.activeID = lane.id
    }

    /// Rewrite the agent instructions to the template standard, in a mode that may edit. The
    /// same persona and evidence as the review, so the docs match what it found.
    func fixDocs() async {
        await attempt {
            let r: ReviewPrompt = try await client.get("/api/review", q())
            mode = "acceptEdits"
            nextSystem = r.system + r.evidence
            prompt = "Rewrite CLAUDE.md and AGENTS.md (and README.md if it misleads) so they are the documents an engineer joining tomorrow and an agent working unsupervised need — derived from the code, not from a template: the gate command and what it runs; a file map of every important directory and entry point; the request path in numbered steps; the invariants this codebase actually has, each with the test that guards it or a note that none does; how to extend it (add a route, a table, a job, a page — files to touch in order); how to run it locally and how it deploys; the ceilings and known problems, honestly. Keep anything true that is already there; delete anything false. Read the code before writing each section. Then run the gate."
            send()
        }
    }

    /// Save the review that just ran into docs/REVIEW.md.
    func saveReview() async -> String? {
        guard let text = reviewLane?.turns.last?.text, !text.isEmpty else { return nil }
        var path: String?
        await attempt { path = try await client.post("/api/review/save", body: SaveBody(text: text), q(), as: String.self) }
        await refreshTree()
        return path
    }
    struct SaveBody: Encodable { var text: String }

    /// A visible rescan: the count after, so the click is seen to do something.
    var scanning = false
    func rescan() async {
        scanning = true
        await refreshState()
        scanning = false
        Telemetry.track("rescanned")
    }

    private var reviewKey: String { "keel.lastReview." + repoPath }
    var lastReview: Date? {
        get { UserDefaults.standard.object(forKey: reviewKey) as? Date }
        set { UserDefaults.standard.set(newValue, forKey: reviewKey) }
    }
    /// A review a day: when a project opens into a lane with nothing in it and the last review
    /// is older than a day, it runs on its own. Never into a conversation already in use.
    func offerReview() {
        guard !repoPath.isEmpty, loaded else { return }
        if let last = lastReview, Date().timeIntervalSince(last) < 86_400 { return }
        if lanes?.lanes.contains(where: { $0.title == "Staff review" && $0.running }) == true { return }
        Task { await requestReview() }
    }
    struct Empty: Encodable {}

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

    /// Which plugin is installing right now, and what it printed.
    var installing: String?
    var installLog = ""

    /// Install a recommended plugin from the panel, and refresh everything the badge reads.
    func installPlugin(_ e: SkillCatalog.Entry) async {
        installing = e.id
        installLog = ""
        defer { installing = nil }
        do {
            for try await ev in client.events("/api/plugins/install",
                                              ["name": e.name, "marketplace": e.marketplace]) {
                if ev.name == "line" || ev.name == "fatal" { installLog += ev.data + "\n" }
            }
        } catch { installLog += error.localizedDescription }
        Telemetry.track("plugin_installed", ["recommended": true])
        await refreshState()
    }

    var tree: [Wire.Node] = []
    /// Every path, flat — for the filter and the mention picker.
    var files: [String] = []

    func refreshTree() async {
        guard let t: [Wire.Node] = try? await client.get("/api/tree", q()) else { return }
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
        // Said out loud from the first moment: the panels go back to "Reading…", and the bar
        // names each step until the setup check is done. Before this, the window sat still
        // for the seconds the daemon took to scan the new repository, then a dialog appeared
        // from nowhere.
        opening = Opening(name: URL(fileURLWithPath: path).lastPathComponent, stage: "opening")
        loaded = false
        _ = try await client.post("/api/open", body: OpenBody(path: path), as: Opened.self)
        projectOpenKnown = true
        Telemetry.track("project_opened")
        Telemetry.breadcrumb("project opened")
        turns.removeAll()
        sessionId = nil
        title = "New feature"
        await refreshGit()
        await refreshState()
        await refreshTree()
        await refreshDev()
    }

    /// The open repository's path, for the project menu and for the Finder.
    var repoPath = ""

    /// A project switch in progress: what is being opened and which step is running.
    struct Opening: Equatable { var name: String; var stage: String }
    var opening: Opening?
    /// The project that just finished opening, shown for a moment so the change is seen.
    var justOpened: String?

    func stage(_ s: String) { if opening != nil { opening?.stage = s } }
    func finishedOpening() {
        guard let o = opening else { return }
        opening = nil
        justOpened = o.name
        Task { try? await Task.sleep(for: .seconds(2.5)); if justOpened == o.name { justOpened = nil } }
    }

    func refreshState() async {
        guard let s: Wire.State = try? await client.get("/api/state") else { return }
        loaded = true
        projectOpenKnown = s.projectOpen
        repoPath = s.repo
        // A project resumed from prefs was never seen by this app's own recents list, so the
        // switcher would be empty on a fresh install until you opened something a second time.
        if s.projectOpen, !s.repo.isEmpty { Recents.remember(s.repo) }
        sessions = s.workspace.sessions
        findings = s.scan.findings
        scan = s.scan
        workspace = s.workspace
        // The recommendations depend on what is installed, and every path that changes that —
        // install, uninstall, disable, the catalog closing — comes through here. One place,
        // so the badge cannot go stale from a caller that forgot.
        await refreshSuggestions()
    }

    /// Open an existing conversation in this window: replay what it did, then carry on.
    func open(session id: String) async {
        sessionId = id
        let known = sessions.first { $0.id == id }
        title = known?.title ?? "Session " + id.prefix(8)
        sessionCwd = known?.elsewhere != nil ? known?.cwd : nil
        turns.removeAll()
        // What the session actually said, and what it did to the repository — two different
        // endpoints, because the daemon deliberately keeps the listing away from the bodies.
        async let bodies: [Wire.Turn]? = try? client.get("/api/session", sq(["id": id]))
        async let work: SessionWork? = try? client.get("/api/session/work", sq(["id": id]))
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
        _ = try? await client.post("/api/fs/reveal", body: PathBody(path: path), q(), as: PathReply.self)
    }

    /// Delete a file. The daemon moves it to the Trash rather than unlinking it, so this is
    /// recoverable — which is the only reason it is offered without a confirmation.
    func delete(_ path: String) async {
        await attempt { _ = try await client.post("/api/fs/delete", body: PathBody(path: path), q(), as: PathReply.self) }
        await refreshTree()
        await refreshGit()
    }

    func rename(_ path: String, to name: String) async {
        await attempt {
            _ = try await client.post("/api/fs/rename", body: RenameBody(path: path, name: name),
                                      q(), as: PathReply.self)
        }
        await refreshTree()
    }

    /// Run a request whose failure must be seen. A discard that fails and looks like it worked
    /// is the kind of silence that costs someone an afternoon.
    /// What a finished turn cost and whether it was any good — the numbers behind "it went in
    /// circles for ten minutes". Counts only; nothing said or written.
    func reportTurn(_ turn: Turn) {
        var props: [String: Any] = [
            "mode": mode,
            "calls": turn.calls.count,
            "failed_calls": turn.calls.count(where: { $0.failed }),
            "files": turn.files.count,
            "seconds": (turn.durationMS ?? 0) / 1000,
            "replayed": turn.replayed,
            "approvals": approvalsThisTurn,
            "model": claudeModel.isEmpty ? "default" : claudeModel,
        ]
        if let t = turn.tokens {
            props["input_tokens"] = t.input + t.cacheRead + t.cacheWrite
            props["output_tokens"] = t.output
            props["cached"] = Int(t.cached * 100)
        }
        if let c = turn.cost { props["cost_cents"] = Int(c * 100) }
        props["committed"] = turn.commit != nil
        switch turn.gate {
        case .passed: props["gate"] = "passed"
        case .failed(_, let problems): props["gate"] = "failed"; props["problems"] = problems.count
        case .running: props["gate"] = "running"
        case .notRun: props["gate"] = "not_run"
        case .none: props["gate"] = "none"
        }
        Telemetry.track("turn_finished", props)
        // A turn that failed the gate or ran no tools is the shape of a bad experience; a
        // warning so it is findable next to the crashes rather than buried in a funnel.
        if case .failed = turn.gate {
            Telemetry.warn("turn failed the gate", ["mode": mode, "calls": "\(turn.calls.count)"])
        }
        approvalsThisTurn = 0
    }
    var approvalsThisTurn = 0

    private func attempt(_ work: () async throws -> Void) async {
        do { try await work(); lastError = nil } catch {
            lastError = error.localizedDescription
            // The kind of failure, never its text: the text can carry a path.
            let e = error as NSError
            Telemetry.warn("request failed", ["domain": e.domain, "code": "\(e.code)"])
        }
    }

    struct MemoryBody: Encodable { var text: String }
    /// What was just remembered, shown for a moment under the composer.
    var remembered: String?

    func remember(_ text: String) async {
        await attempt {
            _ = try await client.post("/api/memory", body: MemoryBody(text: text), q(), as: String.self)
            remembered = String(text.trimmingCharacters(in: CharacterSet(charactersIn: "# ")).prefix(80))
            Telemetry.track("memory_added")
        }
        await refreshGit()
        Task { try? await Task.sleep(for: .seconds(5)); remembered = nil }
    }

    struct SessionRename: Encodable { var id: String; var title: String }

    /// Name this lane. When it has a session, History gets the same name.
    func rename(to name: String) {
        let t = name.trimmingCharacters(in: .whitespaces)
        guard !t.isEmpty else { return }
        title = String(t.prefix(60))
        if let sessionId { Task { await rename(session: sessionId, to: title) } }
    }

    /// Name a session. Kept in Keel's own store — Claude Code's transcript is not touched to
    /// achieve it.
    func rename(session id: String, to title: String) async {
        struct Ok: Decodable {}
        _ = try? await client.post("/api/session/rename",
                                   body: SessionRename(id: id, title: title), as: Bool.self)
        await refreshState()
    }

    struct GitAct: Encodable { var action: String; var path: String; var hunk: Int? }

    /// Stage, unstage or discard one file.
    ///
    /// `discard` on a tracked file is `git restore`; on an untracked one the daemon moves it to
    /// the Trash rather than unlinking it, so a mistaken discard is recoverable.
    func gitAct(_ action: String, _ path: String, hunk: Int? = nil) async {
        await attempt {
            _ = try await client.post("/api/git/act", body: GitAct(action: action, path: path, hunk: hunk),
                                      q(), as: Bool.self)
        }
        await refreshGit()
        // The diffs on screen are read once per view; bumping this makes them read again.
        diffTick += 1
    }

    /// Bumped when the working tree changed under a diff someone is looking at.
    var diffTick = 0

    // MARK: - Rewind

    struct Snapshot: Decodable { var tree: String }
    struct RestoreBody: Encodable { var tree: String }
    struct Restored: Decodable { var undo: String }

    /// The snapshot that undoes the last restore, while there is one.
    var undoSnapshot: String?

    /// Put every file back to how it was before this turn. The restore photographs the tree
    /// first, so it can itself be undone — a rewind you cannot take back is the version people
    /// file issues about.
    func restore(to turn: Turn) async {
        guard let tree = turn.snapshot else { return }
        await restore(tree: tree)
    }

    func restore(tree: String) async {
        do {
            let r = try await client.post("/api/git/restore", body: RestoreBody(tree: tree),
                                          q(), as: Restored.self)
            undoSnapshot = r.undo
        } catch {
            lastError = error.localizedDescription
        }
        await refreshGit()
        await refreshTree()
        diffTick += 1
    }

    func stopDev() async {
        await attempt { _ = try await client.post("/api/dev/stop", body: [String: String](), as: Bool.self) }
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
        if let s: Wire.GitStatus = try? await client.get("/api/git/status", q()) {
            isRepo = s.isRepo
            branch = s.branch
            changes = s.changes
        }
        commits = (try? await client.get("/api/git/log", q(["n": "20"]))) ?? []
    }

    /// The last commits on this checkout, for the list beside the working tree.
    var commits: [Wire.Commit] = []

    // MARK: - The git client

    var branches: Wire.Branches?
    var gitBusy: String?

    func refreshBranches() async {
        branches = try? await client.get("/api/git/branches", q())
    }

    struct BranchBody: Encodable { var action: String; var name: String }
    struct RemoteBody: Encodable { var action: String; var url: String? = nil }
    struct StageAllBody: Encodable { var stage: Bool }

    /// Any git verb from the panel. Busy while it runs, its output as the error if it fails,
    /// and everything the panels read refreshed afterwards.
    func git(_ what: String, _ work: () async throws -> Void) async {
        gitBusy = what
        defer { gitBusy = nil }
        await attempt(work)
        await refreshGit()
        await refreshBranches()
        await refreshTree()
    }

    func branch(_ action: String, _ name: String) async {
        await git(action) { _ = try await client.post("/api/git/branch", body: BranchBody(action: action, name: name), q(), as: String.self) }
    }
    func remote(_ action: String, url: String? = nil) async {
        await git(action) { _ = try await client.post("/api/git/remote", body: RemoteBody(action: action, url: url), q(), as: String.self) }
        if action == "push" { Telemetry.track("pushed") }
    }
    /// Every uncommitted change, gone: tracked back to HEAD, untracked to the Trash. The
    /// panel asks first and says the counts.
    /// Asked from the panel root, not a row: a dialog presented from a row inside a lazy stack
    /// can vanish with the row, and its button then does nothing.
    var confirmingDiscard = false
    var discarded: String?
    func discardAll() async {
        var counts = [0, 0]
        await git("discard") { counts = try await client.post("/api/git/discard-all", body: Nothing(), q(), as: [Int].self) }
        await refreshState()
        discarded = lastError == nil ? "Discarded \(counts[0]) modified, \(counts[1]) new → Trash" : nil
        Telemetry.track("discarded_all")
        Task { try? await Task.sleep(for: .seconds(6)); discarded = nil }
    }
    struct GitIgnoreBody: Encodable { var path: String }
    /// Stop git watching a file, keeping it on disk — what somebody means when they point at
    /// `.env.local` in the panel.
    func gitIgnore(_ path: String) async {
        await git("ignore") { _ = try await client.post("/api/git/ignore", body: GitIgnoreBody(path: path), q(), as: Bool.self) }
        Telemetry.track("path_gitignored")
    }

    func stageAll(_ stage: Bool) async {
        await git(stage ? "stage" : "unstage") { _ = try await client.post("/api/git/stage-all", body: StageAllBody(stage: stage), q(), as: Bool.self) }
    }
    func commit(_ message: String, all: Bool) async {
        await git("commit") {
            _ = try await client.post(all ? "/api/git/commit" : "/api/git/commit-staged",
                                      body: CommitBody(message: message), q(), as: Bool.self)
        }
        Telemetry.track("commit_made", ["manual": true, "all": all])
    }
    /// The commit whose diff is on screen.
    var viewingCommit: Wire.Commit?

    func commitDiff(_ sha: String) async -> [Wire.Diff] {
        (try? await client.get("/api/git/commit/diff", q(["sha": sha]))) ?? []
    }

    /// Send the branch up. The remote's answer is shown either way.
    var pushing = false
    func push() async {
        pushing = true
        defer { pushing = false }
        do {
            _ = try await client.post("/api/git/push", body: Nothing(), q(), as: String.self)
            lastError = nil
            Telemetry.track("pushed")
        } catch { lastError = "Push failed: " + error.localizedDescription }
        await refreshGit()
    }

    /// One commit per accepted turn. On by default: a working tree that only grows is what
    /// makes people nervous about an agent, and a row of small commits beside it is what makes
    /// the same work read as progress — and gives every step a place to go back to.
    var autoCommit: Bool {
        get { UserDefaults.standard.object(forKey: "keel.autoCommit") as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: "keel.autoCommit") }
    }

    struct CommitBody: Encodable { var message: String }

    /// Commit what this turn did, if the gate accepted it.
    ///
    /// Only after a pass (or when the project has no gate to say otherwise): a commit of work
    /// the checks rejected is a commit someone has to know to undo. A failed gate leaves the
    /// changes where they are, red, with the problems listed.
    private func commitTurn(_ turn: Turn) async {
        guard autoCommit, isRepo, turn.didWork, !changes.isEmpty else { return }
        switch turn.gate {
        case .passed, .none, .notRun: break
        case .failed, .running: return
        }
        let first = turn.prompt.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .first { !$0.isEmpty && !$0.hasPrefix("@") } ?? "agent turn"
        let subject = String(first.prefix(72))
        let body = "Made by the agent in Keel, lane \"\(title)\". The project's checks "
            + (turn.gate == .notRun ? "were not run." : "passed.")
        do {
            let committed = try await client.post("/api/git/commit",
                                                  body: CommitBody(message: subject + "\n\n" + body),
                                                  q(), as: Bool.self)
            await refreshGit()
            // Only a commit that happened is this turn's; otherwise the footer would show the
            // previous one as if it were new.
            if committed { turn.commit = commits.first?.sha }
        } catch {
            lastError = "Could not commit: " + error.localizedDescription
        }
    }

    private func gateName(_ g: Turn.Gate) -> String {
        switch g {
        case .passed: "passed"
        case .failed: "failed"
        case .none: "no_gate"
        case .notRun, .running: "not_run"
        }
    }

    /// Take the last commit apart, keeping its changes. `--soft` on the daemon's side.
    func uncommit() async {
        await attempt { _ = try await client.post("/api/git/uncommit", body: Nothing(), q(), as: Bool.self) }
        await refreshGit()
    }

    struct Nothing: Encodable {}

    func gitInit() async -> String? {
        do {
            _ = try await client.post("/api/git/init", body: Nothing(), q(), as: Bool.self)
            await refreshGit()
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    func diff(_ path: String) async -> Wire.Diff? {
        try? await client.get("/api/git/diff", q(["path": path]))
    }
}
