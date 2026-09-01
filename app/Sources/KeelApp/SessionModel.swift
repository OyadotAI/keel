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
    /// Why the last state refresh failed, when it did.
    ///
    /// `loaded` alone could only say "not yet". Every panel refresh was a `try? … else { return }`,
    /// so a daemon that had stopped answering rendered as a permanently empty Files, Git,
    /// Readiness or Plugins panel — which reads as "your project has none of these".
    var loadFailed: String?

    var turns: [Turn] = []
    var running = false
    /// The turn's stream has ended but the tree is not finished with.
    ///
    /// `endTurn` clears `running` first and only then runs the gate — minutes — and the
    /// auto-commit, which is `git add -A` over the whole checkout. For that whole window the
    /// lane did not count as writing, so a second shared lane could start and have its
    /// half-written files committed under this lane's prompt. Read by `writingElsewhere`.
    var settling = false

    /// When the stream last said anything; the working bar reads silence off it.
    /// When anything last arrived on the stream, heartbeat included. Proof the daemon is there.
    var lastEventAt = Date()
    /// When the *agent* last said something. A turn thinking for four minutes is normal; a turn
    /// whose stream has stopped arriving is not, and these used to be the same number.
    var lastProgressAt = Date()
    /// Nothing at all for this long, with a heartbeat every fifteen seconds, is a dead stream.
    static let deadStream: TimeInterval = 60
    /// So the fresh-conversation retry happens once and cannot become a loop.
    private var retriedFresh = false

    /// What the turn is doing *before* the agent starts, when there are no events yet.
    ///
    /// An isolated turn makes a checkout of the repository first, and `git worktree add` copies
    /// the working tree — on a real repository that is minutes, during which the bar said
    /// "thinking…" and then, at ninety seconds, told the person to stop and use the terminal.
    /// It was neither thinking nor stuck; it was checking out, and saying so is the whole fix.
    var preparing: String?
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

    /// A path as the checkout that will serve its diff knows it.
    ///
    /// The agent reports absolute paths. A lane's files live at `<project>/.keel/worktrees/<lane>/…`
    /// while every checkout-scoped request — the diff among them — is answered from the lane's own
    /// checkout, where that prefix does not exist. Stripping the project root first left
    /// `.keel/worktrees/<lane>/…`, and the marker that was meant to remove it required a leading
    /// slash it no longer had. So every file a lane wrote asked git for a path that is not in that
    /// checkout, and the pane reported it as matching HEAD: a lane's work could never be reviewed.
    /// The lane's own prefix goes first now, and matches with or without the leading slash.
    func repoRelative(_ path: String) -> String {
        if let wt = worktree, let cut = path.range(of: ".keel/worktrees/\(wt)/") {
            return String(path[cut.upperBound...])
        }
        if !repoPath.isEmpty, path.hasPrefix(repoPath + "/") {
            return String(path.dropFirst(repoPath.count + 1))
        }
        return path
    }

    /// The files the agent wrote in this conversation, newest turn last, as the Changes panel
    /// shows them — what happened here, not git's view of the working tree (that is the Git tab).
    var editedThisSession: [Wire.Change] {
        var seen: [String: Wire.Change] = [:]
        var order: [String] = []
        for turn in turns {
            for call in turn.calls where Turn.writeTools.contains(call.tool) {
                let path = repoRelative(call.subject)
                guard !path.isEmpty, !path.hasPrefix("/") else { continue }
                // A scratch file the agent wrote and deleted again is not a change to review. It
                // used to sit here for the rest of the session, above a diff with nothing in it.
                // The daemon runs on this Mac, so the file is simply there or it is not.
                if call.subject.hasPrefix("/"),
                   !FileManager.default.fileExists(atPath: call.subject) {
                    seen[path] = nil
                    continue
                }
                let first = seen[path] == nil
                if first, !order.contains(path) { order.append(path) }
                seen[path] = Wire.Change(path: path, status: call.tool == "Write" && first ? "A" : "M",
                                         label: call.tool == "Write" ? "written" : "edited")
            }
        }
        // What git says now wins over what the tool call said then. Keel commits a passing turn by
        // itself, so a file this list still called "written" had usually been committed minutes
        // ago — the panel disagreeing with the repository about the same file.
        // ponytail: a file the repository ignores is never in `changes` either, so it reads as
        // committed; its diff still shows the whole file, and telling the two apart would need a
        // tracked/untracked round trip per row.
        let pending = Dictionary(changes.map { ($0.path, $0) }, uniquingKeysWith: { a, _ in a })
        return order.compactMap { path in
            guard var change = seen[path] else { return nil }
            guard isRepo else { return change }
            if let live = pending[path] {
                change.status = live.status
                change.label = live.label
            } else {
                change.status = "C"
                change.label = "committed"
            }
            return change
        }
    }
    var pending: [Wire.Pending] = []
    /// Every failure the person is shown passes through here, from ~25 call sites. The reporting
    /// hangs off the property rather than off `fail(_:)` so a future `lastError = …` cannot
    /// quietly skip it — which is exactly how none of this was reaching Sentry before.
    var lastError: String? {
        didSet {
            guard let message = lastError, message != oldValue else { return }
            Telemetry.failure(failureCategory, message,
                              ["mode": mode, "provider": provider.queryValue,
                               "fixable": lastFix == nil ? "no" : "yes"])
        }
    }

    /// What kind of failure the next `lastError` is, set by whoever is about to report one.
    /// Defaults to the generic bucket so an unlabelled failure still arrives.
    var failureCategory = "app" 
    /// What to do about `lastError`. Always set through `fail(_:fix:)` so a failure without a next
    /// step is a visible omission rather than the default.
    var lastFix: Fix?

    /// Report a failure with the thing that fixes it.
    ///
    /// The repository already requires every scanner finding to carry a `Fix` — "a finding without
    /// one turns the report into a lint run nobody acts on". A runtime failure is the same: a wall
    /// with no next step is where people stop.
    func fail(_ message: String, category: String = "app", fix: Fix? = nil) {
        lastFix = fix
        failureCategory = category
        lastError = message
    }

    /// A failure Claude Code reported about itself, and what to do about it.
    ///
    /// Keyed on `model == "<synthetic>"` — the marker on every client-generated error turn, in the
    /// live stream and in a replayed transcript alike — with `error` naming which one. None of
    /// these fields was decoded before, so all of them rendered as ordinary prose from the agent.
    struct Trouble {
        var message: String
        var kind: String
        var fix: (SessionModel) -> Fix?
    }

    static func classify(_ r: Record) -> Trouble? {
        let synthetic = r.message?.model == "<synthetic>"
        let flagged = (r.is_api_error_message ?? r.isApiErrorMessage) == true
        guard synthetic || flagged || r.error != nil else { return nil }
        let said = (r.message?.content ?? [])
            .compactMap { $0.type == "text" ? $0.text : nil }
            .joined(separator: " ")
            .trimmingCharacters(in: .whitespacesAndNewlines)

        switch r.error {
        case "authentication_failed", "oauth_org_not_allowed":
            return Trouble(message: said.isEmpty ? "Claude Code is not logged in." : said,
                           kind: r.error ?? "auth") { _ in
                Fix(label: "Log in") {
                    NotificationCenter.default.post(name: .keelRunInTerminal,
                                                    object: "claude /login")
                }
            }
        case "rate_limit":
            var message = said.isEmpty ? "You have hit your usage limit." : said
            if let at = r.quotaLimits?.resetsAt {
                let when = Date(timeIntervalSince1970: at)
                message += " Work can resume "
                    + when.formatted(date: .omitted, time: .shortened) + "."
            }
            return Trouble(message: message, kind: "rate_limit") { model in
                Fix(label: "Retry") { model.retryLast() }
            }
        case "server_error":
            return Trouble(message: said.isEmpty ? "The API had a server error." : said,
                           kind: "server_error") { model in
                Fix(label: "Retry") { model.retryLast() }
            }
        default:
            guard !said.isEmpty else { return nil }
            return Trouble(message: said, kind: r.error ?? "api_error") { _ in nil }
        }
    }

    /// Send the last prompt again, on the same lane.
    func retryLast() {
        guard !running, let last = turns.last else { return }
        lastError = nil; lastFix = nil
        start(last.prompt)
    }

    /// The fix for a repository problem, when there is one.
    ///
    /// A folder that is not a repository is the common case behind "the isolated checkout could
    /// not be created" — `git worktree add` cannot branch from a tree with no HEAD — and Keel can
    /// simply make it one.
    var repoFix: Fix? {
        // Never in a workspace. `git init` at a folder holding `backend/` and `frontend/` makes a
        // third repository *around* two existing ones, which then sees them as untracked
        // directories — the fix would be the damage.
        guard !isRepo, !isWorkspace else { return nil }
        return Fix(label: "Initialise git") { [weak self] in
            Task { @MainActor in
                guard let self else { return }
                if let problem = await self.gitInit() {
                    self.fail("Could not initialise a git repository here: " + problem)
                } else {
                    self.lastError = nil; self.lastFix = nil
                    self.lastFix = nil
                }
            }
        }
    }

    /// Notes left on the page, Figma-style: each is an element and what to do about it. They
    /// stack, they stay pinned on the page across hot reloads, and they go with the next send.
    struct Pin: Identifiable {
        let id = UUID()
        var picked: Picked
        var before: NSImage?
        var note = ""
        /// What you changed by hand on the page, in the units the source uses — `width 240px →
        /// 320px`. Keel does not write the file; it says precisely what you meant and lets the
        /// pixel check afterwards prove the source now matches.
        var nudges: [String] = []
    }
    var pins: [Pin] = []

    /// The pins the turn in flight was sent with, so the after-photos know what to re-shoot.
    var designInFlight: [Pin] = []
    /// Asks the preview to photograph a rect of the page as it is now. Set by the pane while open.
    var resnapshot: ((Picked.Rect) async -> NSImage?)?
    /// Asks the page where these selectors are *now*. Set by the pane while open.
    var rectsNow: (([String]) async -> [String: Picked.Rect])?
    /// Sends a message to the canvas script in the page. Set by the pane while open.
    var canvas: (([String: Any]) -> Void)?
    /// Which pane's coordinator installed the three closures above.
    ///
    /// Popping the preview out means two panes exist for a moment, and the old one's teardown runs
    /// on a `Task` — so without this it could nil out the *new* window's closures and leave the
    /// check with nothing to photograph through.
    weak var canvasOwner: AnyObject?

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
        show(page: path)
        // If a pin's likely source is this file, that pin's element is what is about to move.
        if let pin = pins.first(where: { $0.picked.hints.contains { path.hasSuffix(
            $0.value.split(separator: ":").first.map(String.init) ?? $0.value) } }) {
            canvas?(["keel": "outline", "selector": pin.picked.selector])
        }
        canvas?(["keel": "expect"])
        if followEdits { designTick += 1 }
    }

    /// Point the preview at the page this file is on.
    ///
    /// A page file answers immediately. Anything else is a walk up the import graph, which needs
    /// the daemon and so cannot be synchronous — the navigation lands a moment after the bar says
    /// what is being edited, which is the right way round: the name of the file is known at once
    /// and the page it is on is not.
    private func show(page path: String) {
        if let route = Frontend.route(for: path) {
            navigatePreview(to: route)
            return
        }
        if let cached = pageOfFile[path] {
            navigatePreview(to: cached)
            return
        }
        guard previewURL != nil else { return }
        Task { [client] in
            guard let route = await Frontend.page(containing: path, client: client, query: q())
            else { return }
            self.pageOfFile[path] = route
            // Only if this file is still the one being written: a slow walk must not yank the
            // preview to a page two edits ago.
            if self.editing == path { self.navigatePreview(to: route) }
        }
    }

    private func navigatePreview(to route: String) {
        guard let url = previewURL, let origin = Frontend.origin(of: url),
              url != origin + route else { return }
        previewURL = origin + route
    }

    /// Which page each file turned out to be on. The walk reads files, and an agent editing the
    /// same component six times in a turn should pay for that once.
    private var pageOfFile: [String: String] = [:]

    /// The page a file being written shows on, for the bar that says what is happening.
    func pageShowing(_ path: String) -> String? {
        Frontend.route(for: path) ?? pageOfFile[path]
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
                var design = turn.design ?? Turn.Design()
                design.regions = regions
                design.pageAfter = shot
                turn.design = design
            }
        }
    }

    func clearRegions() {
        changedRegions = []
        // What you dragged was a way of saying it, not the change itself. Put the page back, so
        // what you are looking at when the turn lands is the agent's work and not your ghost.
        canvas?(["keel": "revert"])
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
    /// Where it runs, when that is not the repository root — which for a monorepo it never is.
    var devDir: String?
    var devRunning = false
    var picking = false
    /// The preview is open in a window of its own, so the tab stands aside. Two panes would each
    /// install their own `resnapshot` and `canvas`, and the check would photograph whichever one
    /// happened to win.
    var detachedPreview = false

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
        // Pick stays armed. It disarmed after one click, so pinning three things meant reaching
        // for the toggle twice for no reason — Esc in the page turns it off, and so does the
        // toggle.
        syncCanvas()
    }

    /// A change made by hand in the page: it lands on the pin for that element, making one if
    /// there is none, so dragging a handle is a complete instruction on its own.
    func designNudge(_ p: Picked, label: String) {
        if let i = pins.firstIndex(where: { $0.picked.selector == p.selector }) {
            pins[i].picked = p
            if !pins[i].nudges.contains(label) { pins[i].nudges.append(label) }
        } else {
            pins.append(Pin(picked: p, before: nil, nudges: [label]))
            Telemetry.track("nudge", [:])
        }
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
            if !pin.nudges.isEmpty {
                out += "\n\nI changed this by hand in the running page, as a way of showing you "
                    + "what I want. Make the source produce this — do not add inline styles:\n"
                for n in pin.nudges { out += "  - \(n)\n" }
            }
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
    let id: UUID

    enum Provider: String, Codable, CaseIterable {
        case claude = "Claude Code"
        case codex = "Codex"

        var queryValue: String {
            switch self {
            case .claude: "claude"
            case .codex: "codex"
            }
        }

        /// For the tab, where there is room for a badge and not for a sentence. Now the fallback
        /// for [`ProviderMark`] when the icon file is not where it should be — a tab that says
        /// nothing about which agent is behind it is worse than one that says it in four letters.
        var short: String {
            switch self {
            case .claude: "Claude"
            case .codex: "Codex"
            }
        }

        /// The CLI's own icon, shipped in the app bundle. A tab identifies its agent the way a
        /// browser tab identifies its site.
        var iconResource: String {
            switch self {
            case .claude: "provider-claude"
            case .codex: "provider-codex"
            }
        }

        /// What the model picker offers, which is not the same list for both.
        ///
        /// An empty value means "whatever the CLI's own config says", and it is the default for
        /// exactly that reason: a hardcoded list cannot know about a model somebody put in their
        /// `~/.codex/config.toml`, and offering a name their account cannot use is worse than
        /// offering none. Everything else here is a name that CLI answers to.
        var models: [(value: String, label: String)] {
            switch self {
            case .claude:
                [("", "Default"), ("opus", "Opus"), ("sonnet", "Sonnet"), ("haiku", "Haiku")]
            case .codex:
                [("", "Default"), ("gpt-5-codex", "GPT-5 Codex"), ("gpt-5", "GPT-5"), ("o3", "o3")]
            }
        }
    }

    var provider: Provider = .claude {
        didSet {
            // The names do not transfer. Carrying "opus" onto a Codex lane asks `codex` for a
            // model it has never heard of, which is the bug this whole pass started from.
            if oldValue != provider { claudeModel = "" }
        }
    }
    var scopeFileLimit = 8
    var policySources: [String] = []
    var allowedProviders: Set<String> = ["claude", "codex"]
    var policyRequiresIsolation = true
    /// A policy file demanded isolation. Keel's own default does not count — see `Policy`.
    var isolationByPolicy = false

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
            repos = other.repos
            changes = other.changes
            changesCollapsed = other.changesCollapsed
            tree = other.tree
            files = other.files
        }
        repoPath = other.repoPath
        // With the data, not without it. Every panel keys "not yet" off `loaded`, and a lane that
        // adopted 158 sessions and a scan while `loaded` stayed false drew "Reading…" over all of
        // it — for good, because a new lane never calls `refreshState` and the refreshes that
        // could set it are themselves guarded by it. It was every panel of every tab but the
        // first, from the moment the tab opened.
        loaded = other.loaded
        loadFailed = other.loadFailed
        slashCommands = other.slashCommands
        sessions = other.sessions
        findings = other.findings
        // With the findings, not without them: the panel keys "has this been scanned" off `scan`,
        // so a lane that adopted the findings alone said "Readiness not checked" above a list of
        // findings it was already holding.
        scan = other.scan
        workspace = other.workspace
        trusted = other.trusted
        gateCommand = other.gateCommand
        previewWidth = other.previewWidth
        devDetected = other.devDetected
        devDir = other.devDir
        // Not the running server: that belongs to one checkout, and this copied it to every lane
        // — including the ones the block above deliberately excluded from `branch`, `changes` and
        // `tree` because they have a checkout of their own. `refreshDev` asks for this lane.
        if worktree == nil || worktree == other.worktree {
            previewURL = other.previewURL
            devRunning = other.devRunning
            devElsewhere = other.devElsewhere
        }
        missingSuggestions = other.missingSuggestions
        tools = other.tools
        projectOpenKnown = other.projectOpenKnown
        scopeFileLimit = other.scopeFileLimit
        policySources = other.policySources
        allowedProviders = other.allowedProviders
        policyRequiresIsolation = other.policyRequiresIsolation
        isolationByPolicy = other.isolationByPolicy
    }

    init(client: Client, port: UInt16 = 7777, sessionId: String? = nil, id: UUID = UUID()) {
        self.client = client
        self.port = port
        self.sessionId = sessionId
        self.id = id
        // Here, and not at the first turn. See `watchMonitors`.
        watchMonitors()
    }

    /// Why this feature cannot be merged yet, or `nil`.
    ///
    /// Judged from the branch where it can be, not from what this process happened to watch. Two
    /// states used to be unreachable once entered:
    ///
    /// * **After a relaunch.** Every restored turn is marked `replayed`, so requiring an
    ///   unreplayed one meant a lane holding a finished feature branch reported "no recorded
    ///   implementation work" with Finish greyed out — for good, short of running another turn.
    ///   The commits are on the branch either way, and the daemon already counts them.
    /// * **After a failed gate that was later fixed.** This looped over *every* work turn and
    ///   returned on the first failure, so turn 3 failing blocked the lane even once turn 7 had
    ///   fixed it and passed. What the gate said about the code as it stands is the last one.
    var mergeBlocker: String? {
        if running { return "The agent is still working." }
        if settling { return "The turn is still finishing — the gate, then the commit." }
        if !pending.isEmpty { return "The agent is waiting for a decision." }
        if isolated && worktree == nil { return "The isolated checkout is unavailable." }
        // Recorded turns, or — for a lane reopened from history, whose turns are all replays —
        // commits on the branch and files not yet committed. Work is work whether or not this
        // process was running when it was done.
        guard turns.contains(where: { $0.didWork && !$0.replayed }) || hasWork else {
            return worktree == nil
                ? "Nothing has been changed yet."
                : "This feature has no work on its branch yet."
        }
        if editedThisSession.count > scopeFileLimit {
            return "Scope exceeded the approved \(scopeFileLimit)-file budget. Split or explicitly reduce the task before merging."
        }
        // A gate result is only worth what it was run against. Anything uncommitted arrived after
        // the checks did — a hand edit, or a file the agent left behind — so the recorded verdict
        // is about a tree that no longer exists.
        if !changes.isEmpty, !gateIsCurrent {
            let n = changes.count
            return "\(n) uncommitted change\(n == 1 ? "" : "s") since the checks last ran. "
                + "Commit them; the checks run again before the merge."
        }
        switch latestGate {
        case .passed: return nil
        case .failed: return "The project's quality gate failed on the last turn."
        // Not a refusal. Keel never invents a check, so a project that declares none is not one
        // Keel will not let you merge — it is one where the verdict says so instead of showing a
        // tick nobody earned. `noChecksDeclared` is what puts that on the screen and the button.
        case .none: return nil
        case .running: return "The project quality gate is still running."
        case .notRun:
            return "The checks have not run on this work yet."
        }
    }

    /// The gate whose verdict this lane is judged by.
    ///
    /// The last implementation turn's, falling back to the last turn of any kind — which is what a
    /// reopened lane has, since replaying a conversation produces turns that did no work. Without
    /// the fallback, pressing Run checks on a reopened lane ran the project's whole suite and
    /// changed nothing on screen, because the verdict was reading a turn the result never reached.
    var latestGate: Turn.Gate {
        (turns.last(where: { $0.didWork && !$0.replayed }) ?? turns.last)?.gate ?? .notRun
    }

    /// The checks are running right now, so nothing should offer to start them again.
    var isRunningGate: Bool {
        if case .running = latestGate { return true }
        return false
    }

    /// The project declares no check of its own, so there is nothing that could have passed.
    var noChecksDeclared: Bool {
        if case .none = latestGate { return true }
        return false
    }

    var readyToMerge: Bool { mergeBlocker == nil }

    /// Whether anything has been done in this lane at all.
    ///
    /// Asked of the checkout when there is one, and of the working tree when there is not. It used
    /// to be asked *only* of the checkout, so a shared lane — which has none — reported that its
    /// branch was empty while the same screen listed the files it had touched. That answer also
    /// reached `readyToMerge`, the tab menu's Finish item and `Lanes.finish`'s own refusal, so all
    /// four were wrong together: the reason this is one property rather than a check at each of
    /// them.
    var hasWork: Bool {
        if let checkout = lanes?.worktree(of: self) {
            return checkout.ahead > 0 || checkout.dirty
        }
        return !changes.isEmpty
    }

    /// Whether the recorded gate is about the tree as it stands now.
    ///
    /// The gate runs before the auto-commit, so "the last implementation turn passed, and nothing
    /// has changed since" is the whole of it. A green tick about a tree that no longer exists is
    /// worse than no tick at all, because it is one somebody would act on.
    var gateIsCurrent: Bool {
        guard case .passed = latestGate else { return false }
        return changes.isEmpty
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

    /// Everything that means "there is more below", as one value — for both panes that follow it.
    ///
    /// It lives here rather than in each pane because each pane had its own copy and each copy
    /// missed something that grows: the conversation missed `thinking`, so a long reasoning block
    /// scrolled out of sight, and the Trace missed the live call's output and its subagent's
    /// rows, which is most of what a running turn produces. A count that does not change fires no
    /// `onChange`, so the pane sat still under a stream you then had to scroll to by hand.
    ///
    /// Over-firing is harmless — a scroll to the end of a pane already at the end is a no-op —
    /// so this is deliberately the union of both panes rather than two precise halves.
    ///
    /// Output is summed over every call rather than read off the last one, because results do not
    /// arrive in the order the calls were made: three parallel `Read`s land newest-first as often
    /// as not, and a token watching only the newest misses the two that grew the card. Counted in
    /// `utf8`, which is O(1) on a native string — `count` walks characters, and this is evaluated
    /// on every delta of a turn that can print megabytes.
    var tailToken: String {
        let last = turns.last
        let printed = last?.calls.reduce(0) { n, c in
            n + c.output.utf8.count + c.children.reduce(0) { $0 + $1.output.utf8.count }
        } ?? 0
        let nested = last?.calls.reduce(0) { $0 + $1.children.count } ?? 0
        return [turns.count, last?.text.utf8.count ?? 0, last?.thinking.utf8.count ?? 0,
                last?.calls.count ?? 0, last?.files.count ?? 0,
                printed, nested, running ? 1 : 0]
            .map(String.init).joined(separator: "-")
    }

    func send() {
        var text = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        // Pins with notes are a request on their own; the box may stay empty. So is a pin you
        // dragged: the delta says what you want more precisely than a sentence would.
        if text.isEmpty, pins.contains(where: { !$0.note.isEmpty || !$0.nudges.isEmpty }) {
            text = "Make the source match the pinned elements above."
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
        // A command Keel owns never reaches the agent. Everything else is sent verbatim, because
        // Claude Code resolves slash commands itself — locally, for no tokens and no turn.
        if runOwnCommand(text.trimmingCharacters(in: .whitespacesAndNewlines)) { return }
        guard !running else { queued.append(text); return }
        start(text)
    }

    /// One turn at a time, per lane — enforced here rather than at the call sites.
    ///
    /// Four places call this, and each guarded `running` in its own way: `send` queues instead,
    /// `retryLast` has a `guard !running`, `deliver` branches, `endTurn` calls it only after
    /// setting `running` false. Every one of them is correct today. But the invariant they are
    /// each half of is *this function's* — a second `claude -p` in one lane means two agents
    /// editing one checkout with one of them untracked by Stop, which is the same corruption as
    /// two lanes on one tree and harder to see. An invariant kept by convention at four call
    /// sites is one refactor from being kept at three.
    ///
    /// The prompt is not dropped: it goes where a prompt sent during a turn always goes.
    private func start(_ text: String) {
        guard !running else {
            queued.append(text)
            return
        }
        // Belt and braces on the same invariant: whatever the last turn left behind is finished
        // with, and a cancelled task that is somehow still running must not outlive its lane.
        streamTask?.cancel()
        guard allowedProviders.contains(provider.queryValue) else {
            lastError = "\(provider.rawValue) is not allowed by the active team policy."
            releaseQueued()
            return
        }
        // Two lanes writing one working tree is the one kind of concurrency Keel will not run.
        //
        // Both of the things that end a turn are tree-wide: auto-commit is `git add -A` in the
        // checkout, and rewind restores a whole tree. So whichever turn finishes first sweeps the
        // other's half-written files into a commit labelled with the wrong prompt, and the second
        // lane's own commit then shows less than it did — reproducibly, with nothing on screen
        // saying it happened. Reading and planning beside a lane that is editing is exactly what a
        // shared lane is for; writing beside one is not.
        //
        // Said rather than guessed. Silently moving the work onto a branch would be a surprise
        // about where it landed, and this audience would rather be told and handed the button.
        if mode != "plan", !isolated, let other = lanes?.writingElsewhere(than: self) {
            fail("\(other) is editing this working tree. Two lanes writing the same checkout "
                 + "commit each other's half-finished files, so this one has not started.",
                 category: "shared_tree",
                 fix: Fix(label: "Give this one its own branch") {
                     self.isolated = true
                     self.start(text)
                 })
            releaseQueued()
            return
        }
        if mode != "plan", policyRequiresIsolation { isolated = true }
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
        // What you dragged was a way of saying it, not the change itself. Put the page back, so
        // what you are looking at when the turn lands is the agent's work and not your ghost.
        canvas?(["keel": "revert"])
        canvas?(["keel": "clear"])

        let full = promptWithAttachments(instruction)
        // Counted before they are cleared. Read after `removeAll()`, this had always logged 0.
        let attached = attachments.count
        attachments.removeAll()
        Telemetry.track("turn_started", ["mode": mode, "pins": designInFlight.count,
                                         "attachments": attached])
        Telemetry.breadcrumb("turn started")
        // Typing into a lane that was following a conversation elsewhere takes it over: from here
        // the turns are Keel's own, and two readers appending to one `turns` array would
        // interleave them.
        //
        // Here rather than inside the task below, which is where it used to be: making an
        // isolated checkout comes first there and takes seconds, and the poll kept appending
        // turns of its own for the whole of it — including one for the prompt this turn is
        // already drawing.
        unfollow()
        owned = true
        let turn = Turn(prompt: full)
        turns.append(turn)
        stallReported = false; lastEventAt = Date(); lastProgressAt = Date(); running = true
        lastError = nil; lastFix = nil
        watchApprovals(true)
        watchMonitors()

        streamTask = Task { [client] in
            // An isolated task gets its checkout before the provider starts. Failure stops the
            // task; silently sharing the project tree would break the task's safety contract.
            if isolated, worktree == nil {
                self.preparing = "making an isolated checkout of the repository…"
                let made = await self.makeWorktree()
                self.preparing = nil
                if !made {
                    // A policy file that demands isolation is the one case where not having it is
                    // a reason to refuse. Otherwise the work happens: isolation is Keel's default,
                    // not something the person asked for, and a customer whose worktree could not
                    // be created was blocked from doing anything at all.
                    if self.isolationByPolicy {
                        turn.finished = true
                        running = false
                        watchApprovals(false)
                        self.releaseQueued()
                        return
                    }
                    turn.notIsolated = self.worktreeFailure ?? "the checkout could not be created"
                    self.worktreeFailure = nil
                    self.isolated = false
                    // `makeWorktree` builds the right sentence and the right button for each of
                    // the three causes — "Initialise git", "Make the first commit", "Run in the
                    // project" — and this used to clear both, three lines later. What was left
                    // was a footnote under a finished turn, after the fact, with nothing to
                    // press. The turn still runs; the reason it is not isolated stays on screen.
                    // This lane is now a writer on the shared tree, and the guard that refuses a
                    // second one ran back when `isolated` was still true. Ask again with the
                    // answer that is true now — the daemon refuses this too, but arriving as a
                    // sentence with a button beats arriving as a `fatal`.
                    if self.mode != "plan", let other = self.lanes?.writingElsewhere(than: self) {
                        self.fail("The isolated checkout could not be made, and \(other) is "
                                  + "editing the working tree this turn would fall back to. Two "
                                  + "lanes writing one checkout commit each other's half-finished "
                                  + "files, so this one has not started.",
                                  category: "shared_tree",
                                  fix: Fix(label: "Try its own branch again") {
                                      self.isolated = true
                                      self.start(turn.prompt)
                                  })
                        turn.finished = true
                        self.running = false
                        self.watchApprovals(false)
                        self.releaseQueued()
                        return
                    }
                }
                // Stop pressed while git was copying the tree used to be ignored: `running` went
                // false and the task carried on and started the agent anyway, so the turn people
                // had cancelled ran on invisibly.
                if Task.isCancelled || !self.running {
                    turn.finished = true
                    watchApprovals(false)
                    self.releaseQueued()
                    return
                }
            }
            var query = sq(["prompt": full, "mode": mode, "lane": id.uuidString,
                            "provider": provider.queryValue])
            if !claudeModel.isEmpty { query["model"] = claudeModel }
            if let sessionId { query["session"] = sessionId }
            if let system = nextSystem { query["system"] = system; nextSystem = nil }

            // A new turn is the end of reading an old one. `follow` in the trace pane refuses to
            // scroll while a turn is focused, which is right while you are reading it and wrong
            // the moment you ask for something new.
            self.focusedTurn = nil
            // Before the agent touches anything: what the tree looked like, so "restore to
            // before this turn" has something to restore to. A failure here is not a reason to
            // refuse the turn — there is simply no rewind for it, and the menu says so by absence.
            if let snap: Snapshot = try? await client.post("/api/git/snapshot", body: Wire.None(),
                                                            q(), as: Snapshot.self) {
                turn.snapshot = snap.tree
            }
            do {
                for try await event in client.events("/api/chat", query) {
                    await self.apply(event)
                }
            } catch {
                self.lastError = error.localizedDescription
            }
            // A turn that failed only because the conversation it resumed is gone is worth
            // retrying once, on a fresh one. The alternative is what shipped: an empty card, and
            // the same failure again on every send until the lane is thrown away.
            if turn.failed, self.sessionId == nil, !self.retriedFresh, turn.files.isEmpty {
                self.retriedFresh = true
                self.turns.removeAll { $0 === turn }
                self.running = false
                self.watchApprovals(false)
                self.start(full)
                return
            }
            await self.endTurn(turn)
        }
    }

    /// The conversation Keel was resuming no longer exists.
    ///
    /// It happens after a re-login, a cleared history, or a lane opened on another machine —
    /// and it is self-perpetuating, because Keel kept handing the same dead id to `--resume` on
    /// every retry. The provider prints this on stderr and puts nothing but `is_error` on stdout,
    /// so before the daemon drained stderr there was nothing to notice.
    static func sessionIsGone(_ message: String) -> Bool {
        let m = message.lowercased()
        return m.contains("no conversation found") || m.contains("session id")
            && (m.contains("not found") || m.contains("does not exist"))
    }

    private func endTurn(_ turn: Turn) async {
        close(turn)
        // The composer is free — the agent has stopped talking — but the gate and the commit
        // below still own the tree, so another lane must not start writing it yet.
        settling = true
        defer { settling = false }
        // What the turn wrote through `Bash` rather than through `Edit` — a heredoc, a `tee`, a
        // formatter it ran — is in the tree and in no tool call. See `attribute`.
        let before = treeFingerprint
        await refreshGit()
        await refreshTree()
        attribute(before, to: turn)
        // Written now that the file list is final. Everything on a turn except its prose is
        // Keel's own reading of the machine, and none of it is in the transcript — so without
        // this the next launch rebuilds the turn with its words and none of its work.
        await remember(turn)
        // The turn just changed the repository, and readiness is a reading of the repository.
        // Without this the panel kept the findings from before the fix — including the one the
        // person clicked "Fix this" on — until they went and pressed rescan themselves.
        await refreshState()
        await lanes?.refreshWorktrees()
        // The agent does not grade its own work.
        if mode != "plan" { await runGate(turn) }
        // Before the commit, not after. "The edit went to the wrong file" is a reason not to
        // commit, and it used to arrive after the commit it should have questioned.
        await checkDesign(turn)
        // Accepted work becomes a commit, so the tree stays small and every step is a place
        // to go back to.
        if mode != "plan" { await commitTurn(turn) }
        // The diffs in the turn are read once, when their card appears — which for a file the
        // agent is in the middle of writing is before there is anything to read, and the daemon
        // rightly answered "this file is not on disk any more". Nothing then asked again, so a new
        // file stayed at +0 −0 until the pane was rebuilt by navigating away and back. The tree is
        // final here: gate run, design checked, commit made.
        diffTick += 1
        Notifications.turnFinished(lane: self, files: turn.files.count, gate: turn.gate)
        reportTurn(turn)
        if !queued.isEmpty { start(queued.removeFirst()) }
    }

    /// Give back whatever was queued behind a turn that ended without running it.
    ///
    /// `endTurn` is the only place that drains `queued`, and there are four ways a turn can end
    /// without reaching it: Stop, a policy that demands an isolation the checkout could not give,
    /// a provider the policy forbids, and a cancel while git was still copying the tree. Each of
    /// those left the composer's own promise — "N queued — they run in order when this turn ends"
    /// — pointing at a turn that had already ended, and because `running` was false by then the
    /// next thing typed went straight to `start` and stepped over them for good.
    ///
    /// Back into the composer rather than into the next turn: the person stopped, or Keel refused,
    /// and neither is a reason to run something they have not looked at since. Nothing is lost and
    /// nothing runs unasked.
    private func releaseQueued() {
        guard !queued.isEmpty else { return }
        prompt = (queued + [prompt]).filter { !$0.isEmpty }.joined(separator: "\n\n")
        queued.removeAll()
    }

    /// Interrupt the turn — SIGINT to the agent's process group, the way ⌃C does it.
    ///
    /// The interrupt goes first and the stream is dropped after. Cancelling the stream on its own
    /// was the whole bug: the daemon only learned the client had gone when the *next* line arrived,
    /// so a turn sitting quiet inside a three-minute test kept running while this said it had
    /// stopped. `running` still flips immediately — the process is being signalled, and a button
    /// that waits for a round trip reads as a button that did nothing.
    /// Stop following a session. A stream nobody is looking at still polls a file every 400 ms.
    func unfollow() {
        followTask?.cancel()
        followTask = nil
        watchdogTask?.cancel()
        watchdogTask = nil
        // Put back here, synchronously, rather than in the cancelled task's exit: `start()` takes
        // a followed lane over and sets `running` itself, and a task exiting a moment later that
        // set it false again would end the turn that had just begun.
        if let turn = followed { turn.settle(); turn.finished = true }
        followed = nil
        catchUp = []
        if replay == .reading { replay = .none }
        following = false
    }

    func stop() {
        let lane = id.uuidString
        Task { [client] in
            do {
                _ = try await client.post("/api/chat/stop", body: Wire.None(), ["lane": lane],
                                          as: Stopped.self)
            } catch {
                // Worth saying: the person pressed Stop and the agent may still be running.
                self.lastError = "Could not stop the turn: \(error.localizedDescription)"
            }
        }
        streamTask?.cancel()
        streamTask = nil
        if let turn = current { close(turn) } else { running = false; preparing = nil; watchApprovals(false) }
        releaseQueued()
    }

    struct Stopped: Decodable { var stopped: Bool }

    // MARK: - The checkout

    struct WorktreeName: Encodable { var name: String; var from: String? }
    /// The branch this lane was told to start from, when the person chose one.
    var baseBranch: String?
    /// Why the last checkout attempt failed, for the turn that then ran without one.
    private var worktreeFailure: String?

    /// The branch name the person typed in New Feature, slugged. Empty means "name it for the
    /// first message", which is what this always did.
    var chosenName = ""

    /// Create this lane's checkout, named from what was typed in New Feature or from its title.
    @discardableResult
    func makeWorktree() async -> Bool {
        // A name somebody chose is used as it is. The three random characters are a collision
        // breaker, not part of the name, so they are only added when the name is already taken —
        // a branch called `keel/billing-retries` is one you can find in a `git log`, and
        // `keel/billing-retries-e8e` is one you cannot.
        let wanted = chosenName.isEmpty ? Self.slug(title) : chosenName
        let taken = Set(lanes?.worktrees.map(\.name) ?? [])
        let name = taken.contains(wanted)
            ? wanted + "-" + String(UUID().uuidString.prefix(3)).lowercased()
            : wanted
        do {
            let made: Wire.Worktree = try await client.post("/api/worktree/create",
                                                            body: WorktreeName(name: name, from: baseBranch))
            worktree = made.name
            await refreshGit()
            await refreshTree()
            await lanes?.refreshWorktrees()
            return true
        } catch {
            // Kept rather than shown directly: the caller decides whether this refuses the turn
            // or becomes a note on a turn that ran in the project anyway.
            worktreeFailure = error.localizedDescription
            if !isRepo {
                // The usual cause, and one Keel can fix in a click: `git worktree add` has no HEAD
                // to branch from because the folder was never a repository.
                fail("This folder is not a git repository, so Keel could not make an isolated "
                     + "checkout — and without one there is nothing to branch from or commit to.",
                     category: "not_a_repo", fix: repoFix)
            } else if error.localizedDescription.contains("no commits yet") {
                // A repository nobody has committed to yet has no HEAD to branch from. Keel can
                // make that first commit — which is the thing the person would go and do anyway,
                // and which nothing offered, so this failure repeated turn after turn.
                fail("This repository has no commits yet, so there is nothing to branch from. "
                     + "Keel needs one commit before it can work in an isolated checkout.",
                     category: "no_commits",
                     fix: Fix(label: "Make the first commit") { [weak self] in
                         Task { @MainActor in
                             guard let self else { return }
                             await self.commit("Initial commit", all: true)
                             if self.lastError == nil { self.lastFix = nil }
                         }
                     })
            } else {
                fail("The isolated checkout could not be created: " + error.localizedDescription,
                     category: "worktree",
                     fix: Fix(label: "Run in the project") { [weak self] in
                         self?.isolated = false
                         self?.lastError = nil
                         self?.lastFix = nil
                     })
            }
            return false
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
        var level: String?
        var content: String?
        var subtype: String?
        var session_id: String?
        var model: String?
        var total_cost_usd: Double?
        var duration_ms: Int?
        /// The client-generated failure turns. `error` is a top-level string on an `assistant`
        /// record — `rate_limit`, `server_error`, `authentication_failed`, `oauth_org_not_allowed`
        /// — and the flag is spelled `is_api_error_message` on the live stream but
        /// `isApiErrorMessage` in the transcripts Keel replays, so both are decoded. The reliable
        /// discriminator across both is `message.model == "<synthetic>"`.
        var error: String?
        var is_api_error_message: Bool?
        var isApiErrorMessage: Bool?
        var apiErrorStatus: Int?
        var quotaLimits: Quota?

        struct Quota: Decodable {
            /// Epoch seconds. The one thing a rate-limited person actually wants to know.
            var resetsAt: Double?
            var rateLimitType: String?
        }
        /// On a `result`: whether the run failed outright, and what it said about it. Read from
        /// tool results all along and never from the turn's own result — so a run that failed
        /// drew a card indistinguishable from a quiet success.
        var is_error: Bool?
        var result: String?
        var message: Message?
        var permission_denials: [Denial]?

        struct Denial: Decodable {
            var tool_name: String?
        }
        var event: StreamEvent?
        /// On a `result`: the whole turn's tokens.
        var usage: Usage?
        /// Set on every record a subagent produced: the `Task` call it is working for.
        var parent_tool_use_id: String?
        /// On `system/init`: every slash command this `claude` accepts, which is the only
        /// authoritative list of them — built-ins, the project's own, plugins and skills, exactly
        /// as configured on this machine. Keel guessing at it would be a worse list.
        var slash_commands: [String]?
        /// ISO-8601, on every transcript record. The live stream's `result` carries
        /// `duration_ms`; a transcript has no `result` at all, so a replayed turn's elapsed time
        /// is the span between its first record and its last.
        var timestamp: String?

        struct Message: Decodable {
            /// A transcript's `user` record carries `"content": "hi"` as often as it carries an
            /// array of blocks, and a strict `[Block]?` throws on the string — which fails the
            /// *whole* record, not the field. Following a session on disk, that would have made
            /// every typed message unreadable.
            var content: [Block]?
            init(from decoder: Decoder) throws {
                let c = try decoder.container(keyedBy: CodingKeys.self)
                if let blocks = try? c.decode([Block].self, forKey: .content) {
                    content = blocks
                } else if let text = try? c.decode(String.self, forKey: .content), !text.isEmpty {
                    content = [Block(type: "text", text: text)]
                }
                model = try? c.decode(String.self, forKey: .model)
                stop_reason = try? c.decode(String.self, forKey: .stop_reason)
                usage = try? c.decode(Usage.self, forKey: .usage)
            }
            enum CodingKeys: String, CodingKey { case content, model, stop_reason, usage }
            /// `<synthetic>` marks a turn Claude Code generated itself to report a failure —
            /// the same in the live stream and in a transcript, which no other signal is.
            var model: String?
            var stop_reason: String?
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
            init(type: String, text: String? = nil) { self.type = type; self.text = text }
            var type: String
            /// A complete reasoning block. Live it arrives as `thinking_delta`s, but a transcript
            /// holds it whole — and the reader that only knew the delta dropped every one of
            /// them, which is why a reopened conversation had no reasoning in it at all.
            var thinking: String?
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
            /// Which content block this event is about. A tool call's arguments and the prose
            /// beside it stream interleaved, so the index is how a delta finds its block.
            var index: Int?
            var delta: Delta?
            var content_block: ContentBlock?
        }
        /// A `content_block_start` announces the block before any of it arrives — including, for a
        /// `tool_use`, its id and name. That is what lets a command appear on screen while its
        /// arguments are still being written, which is what the CLI does and Keel did not.
        struct ContentBlock: Decodable {
            var type: String?
            var id: String?
            var name: String?
        }
        struct Delta: Decodable {
            var type: String?
            var text: String?
            var thinking: String?
            /// `input_json_delta`: a fragment of the tool's arguments. Ignored for the whole of
            /// Keel's life so far, which is why a tool call materialised only once it was complete.
            var partial_json: String?
        }
    }

    /// One decoder for the life of the lane. A `JSONDecoder()` per line is an allocation per
    /// token, on the main actor, for the whole of a turn.
    private let decoder = JSONDecoder()

    /// Returns whether this record closed the turn: an `assistant` record that stopped for
    /// `end_turn` is the last thing a turn says, in the live stream and in the transcript alike.
    @discardableResult
    func record(_ data: Data, into turn: Turn) -> Bool {
        if provider == .codex, recordCodex(data, into: turn) { return false }
        // A line Keel cannot read is still a line the agent produced. It used to return here —
        // no log, no counter, nothing on screen — so a shape the CLI changed emptied the UI with
        // no symptom. It is kept *and* counted, so the raw view is the answer rather than a shrug.
        guard let r = try? decoder.decode(Record.self, from: data) else {
            turn.unreadable += 1
            turn.note(raw: String(decoding: data, as: UTF8.self))
            return false
        }
        absorb(r, raw: data, into: turn)
        return r.type == "assistant" && r.message?.stop_reason == "end_turn"
    }

    private func absorb(_ r: Record, raw data: Data, into turn: Turn) {
        // How long a replayed turn took, from the records themselves.
        //
        // A live turn gets `duration_ms` off its `result`; a transcript has no `result`, so the
        // span between the turn's first record and its last is the only measurement there is —
        // and it is a real one rather than a guess.
        if let stamp = r.timestamp, let at = Self.moment(stamp) {
            turn.saw(at)
        }
        // Kept here rather than in the stream loop, and not for the per-token deltas.
        //
        // `rawCap` is 2,000 lines and `--include-partial-messages` emits one line per token, so
        // the escape hatch that exists to guarantee "there is no state in which Keel saw something
        // and you cannot" filled with `content_block_delta` noise and dropped every meaningful
        // record after the first couple of thousand tokens. The deltas are not lost: they are the
        // reply, and the reply is on screen.
        if r.type != "stream_event" { turn.note(raw: String(decoding: data, as: UTF8.self)) }

        switch r.type {
        case "system" where r.subtype == "init":
            let fresh = sessionId == nil && r.session_id != nil
            sessionId = r.session_id ?? sessionId
            if let cs = r.slash_commands, !cs.isEmpty { rememberCommands(cs) }
            // Claude Code only titles a session when its own UI asks for one, so a session
            // Keel drove would list as a bare id. The lane's title is the first ask; it goes
            // into Keel's own name store, which is what History reads.
            if fresh, let id = sessionId, title != "Untitled" {
                Task { await rename(session: id, to: title) }
            }

        case "stream_event":
            guard let e = r.event else { return }
            switch e.type {
            case "content_block_start":
                switch e.content_block?.type {
                case "text":
                    // A new text block after a tool call is a new paragraph. Without this the
                    // second message's first word landed flush against the first message's last.
                    if !turn.text.isEmpty, !turn.text.hasSuffix("\n\n") {
                        turn.text += turn.text.hasSuffix("\n") ? "\n" : "\n\n"
                    }
                    turn.say()
                case "thinking", "redacted_thinking":
                    if !turn.thinking.isEmpty { turn.thinking += "\n\n" }
                    turn.think()
                case "tool_use", "server_tool_use":
                    // The call, before its arguments exist. Everything the row needs to draw
                    // itself — the tool, the glyph, the spinner — is here; the subject fills in
                    // as `input_json_delta` arrives.
                    guard let id = e.content_block?.id, let name = e.content_block?.name else { return }
                    turn.begin(call: id, tool: name, input: [:], parent: r.parent_tool_use_id)
                    if let index = e.index { turn.openBlocks[index] = id }
                default:
                    return
                }

            // Matched on the delta being present rather than on the envelope's own name.
            //
            // `content_block_delta` always carries one, so this is that case — and it is also
            // every future spelling of it. A stream that renames the event, or stops setting
            // `type` at all, would otherwise drop the entire reply with no symptom, which is the
            // failure this file exists to prevent.
            case _ where e.delta != nil:
                guard let d = e.delta else { return }
                switch d.type {
                case "text_delta": if let t = d.text { turn.said(t) }
                case "thinking_delta": if let t = d.thinking { turn.thought(t) }
                case "input_json_delta":
                    guard let json = d.partial_json, let index = e.index,
                          let id = turn.openBlocks[index] else { return }
                    turn.argue(call: id, json: json)
                default:
                    return
                }

            case "content_block_stop":
                guard let index = e.index, let id = turn.openBlocks.removeValue(forKey: index) else { return }
                turn.settle(call: id)

            default:
                return
            }

        case "assistant":
            // A failure Claude Code generated itself, not something the agent said. It used to be
            // appended to `turn.text` and drawn in the agent's own voice, with no action — which
            // is how "Not logged in · Please run /login" arrived looking like a remark.
            if let failure = Self.classify(r) {
                turn.failure = failure.message
                // Recorded on the turn, but not raised.
                //
                // `classify` matches `message.model == "<synthetic>"`, which is exactly what the
                // CLI writes into a transcript — so routing a replay through this reader popped a
                // red banner with a **Retry** button for a rate limit that reset weeks ago, and
                // reported it to Sentry as a turn that had just failed. Seven of them in this
                // project's own history. What already happened is shown on the turn it happened
                // to; only a live failure is something to act on.
                guard !turn.replayed else { return }
                fail(failure.message, category: failure.kind, fix: failure.fix(self))
                // The reason these were not arriving in Sentry is that only the five-minute stall
                // was ever reported. The taxonomy goes; the message never does — it can quote a
                // repository or an organisation name.
                Telemetry.track("turn_failed", ["kind": failure.kind,
                                                "status": r.apiErrorStatus ?? 0,
                                                "mode": mode])
                Telemetry.warn("turn failed: \(failure.kind)", ["kind": failure.kind,
                                                                "mode": mode])
                return
            }
            if let u = r.message?.usage, u.context > 0 { turn.contextTokens = u.context }
            // Accumulated per request, because a transcript has no `result` record.
            //
            // Checked against every `.jsonl` in this project: zero `result` records, and 9,420
            // `assistant` records carrying `message.usage`. Reading the turn total only off
            // `result` meant a session opened from History drew no footer at all — no time, no
            // tokens, no cache share — which is the strip the deleted `session_work` existed to
            // fill. On a live stream `result` still arrives and *assigns* the authoritative
            // total, so this cannot double-count.
            if let u = r.message?.usage, u.context > 0 || (u.output_tokens ?? 0) > 0 {
                turn.tokens = (turn.tokens ?? Turn.Tokens()) + Turn.Tokens(
                    input: u.input_tokens ?? 0, output: u.output_tokens ?? 0,
                    cacheRead: u.cache_read_input_tokens ?? 0,
                    cacheWrite: u.cache_creation_input_tokens ?? 0)
            }
            // Without partial messages the prose arrives only here, whole. Take it when nothing
            // streamed it first.
            // One pass over the blocks, in the order the message holds them.
            //
            // It used to be two — every `text`, then every `tool_use` — which reversed the
            // sequence of any message that spoke, called a tool, and spoke again. Live that never
            // showed, because the deltas had already put the prose in order; replayed, it was the
            // only ordering there was.
            for b in r.message?.content ?? [] {
                switch b.type {
                case "text":
                    // Without partial messages the prose arrives only here, whole — so it has to
                    // open its own block, or the ordered view of the turn would be empty for a
                    // provider, or a flag, that does not stream.
                    if turn.streamedText == false, let t = b.text, !t.isEmpty { turn.wrote(t) }
                case "thinking", "redacted_thinking":
                    // Live this arrives as deltas; a transcript holds it whole. Reading only the
                    // delta is why a reopened conversation had no reasoning in it.
                    if !turn.streamedText, let t = b.thinking, !t.isEmpty { turn.mused(t) }
                case "tool_use":
                    guard let id = b.id, let name = b.name else { continue }
                    turn.begin(call: id, tool: name, input: b.input ?? [:],
                               parent: r.parent_tool_use_id)
                default:
                    continue
                }
            }
            for b in r.message?.content ?? [] where b.type == "tool_use" {
                // The moment the agent starts writing a frontend file, the page is the thing to
                // look at — the loop every vibe-coding tool is criticised for not closing.
                guard let name = b.name, Turn.writeTools.contains(name),
                      let path = b.input?["file_path"]?.stringValue ?? b.input?["path"]?.stringValue,
                      Frontend.isUI(path)
                else { continue }
                frontendEdit(path)
            }

        case "user":
            for b in r.message?.content ?? [] where b.type == "tool_result" {
                guard let id = b.tool_use_id else { continue }
                let output = b.content?.flatText ?? ""
                turn.finish(call: id, output: output, failed: b.is_error ?? false)
                // A file card is drawn when the write *starts* — that is when the path is known —
                // so its diff is read before the file exists, and the daemon truthfully answers
                // that it is not on disk. Now the write has landed, so the cards read again. Keyed
                // to write tools: every other result would be a refetch of everything for nothing.
                if let name = turn.toolName(of: id), Turn.writeTools.contains(name) {
                    diffTick += 1
                    // Here rather than when the call opens: the path is known at the start of a
                    // write and the file only exists at the end of it, and a banner about a file
                    // that is not on disk yet is a banner about nothing. Throttled in
                    // `Notifications`, because a turn writes twenty of these.
                    if let path = turn.files.last {
                        Notifications.fileWritten(lane: self, path: path, count: turn.files.count)
                    }
                }
                noteRefusal(in: output, call: id, turn: turn)
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
            // `is_error` was read from tool results and ignored on the turn's own result, so a
            // run that failed outright drew a card that looked like a quiet success. The text is
            // often absent (a `--resume` against a missing conversation sets the flag and says
            // why on stderr), so the daemon's `fatal` fills that in.
            if r.is_error == true {
                let said = r.result?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                turn.failed = true
                if !said.isEmpty { lastError = said }
            }
            if let c = r.total_cost_usd { turn.cost = c }
            turn.durationMS = r.duration_ms
            if let u = r.usage {
                turn.tokens = Turn.Tokens(
                    input: u.input_tokens ?? 0, output: u.output_tokens ?? 0,
                    cacheRead: u.cache_read_input_tokens ?? 0,
                    cacheWrite: u.cache_creation_input_tokens ?? 0)
            }
            sessionId = r.session_id ?? sessionId

        case "system":
            // Every other subtype. Compaction is the one that matters: the context halves and
            // nothing said so. A `level` of `warning` is Claude Code warning the person directly.
            if r.subtype == "compact_boundary" {
                turn.note(raw: "the conversation was compacted")
            } else if r.level == "warning", let said = r.content, !said.isEmpty,
                      !turn.replayed {
                // Same reason as the synthetic failure above: "Remote Control disconnected — run
                // /login" is a thing that happened during that session, not a thing wrong now.
                fail(said)
            }

        default:
            // Kept, not discarded. The raw line is already recorded; this is the marker that Keel
            // had no interpretation for it.
            turn.unknown.insert(r.type)
        }
    }

    private func recordCodex(_ data: Data, into turn: Turn) -> Bool {
        guard let event = try? JSONDecoder().decode(CodexRecord.self, from: data) else { return false }
        switch event.type {
        case "thread.started":
            sessionId = event.thread_id ?? sessionId
        case "item.started", "item.updated", "item.completed":
            guard let item = event.item else { return true }
            switch item.type {
            case "command_execution":
                if event.type == "item.started" {
                    turn.begin(call: item.id, tool: "Bash",
                               input: ["command": .string(item.command ?? ""),
                                       "description": .string("Command selected by Codex")])
                } else if event.type == "item.completed" {
                    turn.finish(call: item.id, output: item.aggregated_output ?? "",
                                failed: item.status == "failed" || (item.exit_code ?? 0) != 0)
                }
            case "file_change":
                for change in item.changes ?? [] { turn.noteEdit(change.path) }
            case "agent_message":
                if let text = item.text, !text.isEmpty {
                    if !turn.text.isEmpty { turn.text += "\n\n" }
                    turn.text += text
                }
            case "reasoning":
                if let text = item.text { turn.thinking += text }
            case "error":
                lastError = item.message
            default: break
            }
        case "turn.completed":
            if let usage = event.usage {
                turn.tokens = Turn.Tokens(input: usage.input_tokens ?? 0,
                                          output: usage.output_tokens ?? 0,
                                          cacheRead: usage.cached_input_tokens ?? 0,
                                          cacheWrite: 0)
            }
        case "turn.failed", "error":
            lastError = event.error?.message ?? event.message ?? "Codex turn failed"
        default: break
        }
        return true
    }

    struct CodexRecord: Decodable {
        var type: String
        var thread_id: String?
        var item: Item?
        var usage: Usage?
        var error: ErrorBody?
        var message: String?

        struct Usage: Decodable {
            var input_tokens: Int?
            var cached_input_tokens: Int?
            var output_tokens: Int?
        }
        struct ErrorBody: Decodable { var message: String? }
        struct Change: Decodable { var path: String? }
        struct Item: Decodable {
            var id: String
            var type: String
            var command: String?
            var aggregated_output: String?
            var exit_code: Int?
            var status: String?
            var text: String?
            var message: String?
            var changes: [Change]?
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
        if let check: Wire.Check = try? await client.get("/api/verify/plan", q()) {
            gateCommand = check.command
        }
    }

    /// Run the gate against the latest turn, on demand.
    func runGateNow() async {
        // The turn the verdict reads, not merely the last one. On a reopened lane the last turn is
        // a replay, and a gate recorded there is one `mergeBlocker` never looks at — so the button
        // ran the project's whole suite and nothing on screen changed.
        guard let turn = turns.last(where: { $0.didWork && !$0.replayed }) ?? turns.last else {
            return
        }
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
    func checkDesign(_ turn: Turn) async {
        let flight = designInFlight
        designInFlight = []
        guard !flight.isEmpty else { return }

        let seen = changeTick
        for _ in 0..<60 where changeTick == seen && !turn.files.isEmpty {
            try? await Task.sleep(for: .milliseconds(100))
        }
        if changeTick == seen { try? await Task.sleep(for: .milliseconds(400)) }

        var design = turn.design ?? Turn.Design()
        design.duplicated = DesignCheck.looksDuplicated(
            files: turn.files, hints: flight.flatMap(\.picked.hints))
        design.regions = design.regions.isEmpty ? changedRegions : design.regions

        guard let resnapshot else {
            design.pins = flight.map {
                .init(selector: $0.picked.selector, before: $0.before, after: nil,
                      verdict: .notCompared("the Designer was closed, so there was nothing to "
                                            + "photograph"))
            }
            turn.design = design
            return
        }

        // Where the elements are *now*. A rect captured at pick time is a square of the viewport,
        // and anything that scrolled between the click and here would have had the after-shot
        // taken of whatever moved into that square.
        let fresh = await rectsNow?(flight.map(\.picked.selector)) ?? [:]

        var checked: [Turn.Design.Pin] = []
        for pin in flight {
            let selector = pin.picked.selector
            // `rectsNow` is nil only when the pane went away between the guard above and here;
            // an empty answer from a page that did reply means nobody could resolve it.
            guard let rect = fresh[selector] ?? (rectsNow == nil ? pin.picked.rect : nil) else {
                checked.append(.init(selector: selector, before: pin.before, after: nil,
                                     verdict: .notCompared("the element is no longer on the page")))
                continue
            }
            guard rect.width > 1, rect.height > 1 else {
                checked.append(.init(selector: selector, before: pin.before, after: nil,
                                     verdict: .notCompared("the element is no longer visible")))
                continue
            }
            let after = await resnapshot(rect)
            checked.append(.init(
                selector: selector, before: pin.before, after: after,
                verdict: after == nil
                    ? .notCompared("the element is off-screen — scroll it into view to check it")
                    : DesignCheck.compare(before: pin.before, after: after)))
        }
        design.pins = checked
        turn.design = design
    }

    // MARK: - Dev server

    struct DevStatus: Decodable {
        var running: Bool
        var url: String?
        var detected: String?
        /// The subdirectory it runs in, empty at the repository root.
        var detectedDir: String?
        var log: [String]?
        /// The running server belongs to another checkout.
        var elsewhere: Bool?
        /// Which lane, when it does. Empty means the project itself.
        var owner: String?

        enum CodingKeys: String, CodingKey {
            case running, url, detected, log, elsewhere, owner
            case detectedDir = "detected_dir"
        }
    }

    /// The dev server's last lines, for when the page is blank and the reason is in them.
    var devLog: [String] = []
    /// What went wrong loading the page, from the web view itself.
    var previewProblem: String?
    /// Bumped to ask the web view to reload the page it has.
    var reloadTick = 0

    // MARK: - Slash commands

    /// Every slash command this `claude` accepts, as it reported them.
    ///
    /// Read off the `init` record rather than derived: Claude Code knows its own built-ins, the
    /// project's `.claude/commands`, every plugin's, and every skill — 88 of them on a normal
    /// machine — and any list Keel assembled itself would be a worse one that drifts.
    ///
    /// Kept in `UserDefaults` per project so the picker works before the first turn of a session,
    /// which is exactly when somebody reaches for `/`.
    var slashCommands: [String] = []

    private var commandsKey: String { "keel.slashCommands." + repoPath }

    private func rememberCommands(_ list: [String]) {
        slashCommands = list
        UserDefaults.standard.set(list, forKey: commandsKey)
    }

    func loadCommands() {
        guard slashCommands.isEmpty, !repoPath.isEmpty else { return }
        slashCommands = UserDefaults.standard.stringArray(forKey: commandsKey) ?? []
    }

    /// Commands Keel answers itself, because they are about the lane rather than the agent.
    ///
    /// `/clear` is the one that matters: Claude Code clearing its own session would leave Keel
    /// still holding the id and resuming the thing that was just cleared.
    static let ownCommands: [(name: String, detail: String)] = [
        ("clear", "start this task over — new conversation, same branch"),
    ]

    /// Runs a slash command Keel owns. Returns false when it belongs to the agent.
    func runOwnCommand(_ text: String) -> Bool {
        guard text == "/clear" else { return false }
        stop()
        turns.removeAll()
        sessionId = nil
        pending.removeAll()
        queued.removeAll()
        prompt = ""
        Telemetry.track("lane_cleared")
        return true
    }

    func refreshDev() async {
        guard let d: DevStatus = try? await client.get("/api/dev", q()) else { return }
        // Keel runs one dev server. When it is not this checkout's, this lane has no preview —
        // showing the other one's would be reviewing another lane's code against this one's diff,
        // and the design turn's pixel check would photograph it and report a verdict.
        devElsewhere = d.elsewhere == true
            ? (d.owner.flatMap { $0.isEmpty ? "the project" : $0 } ?? "another feature")
            : nil
        devRunning = d.running && !devIsElsewhere
        devDetected = d.detected
        devDir = d.detectedDir.flatMap { $0.isEmpty ? nil : $0 }
        devLog = d.log ?? []
        if devIsElsewhere { previewURL = nil } else if let u = d.url { previewURL = u }
    }

    /// The lane whose dev server is running, when it is not this one's.
    var devElsewhere: String?
    var devIsElsewhere: Bool { devElsewhere != nil }

    struct StartDev: Encodable { var command: String? }

    func startDev() async {
        // The refusal used to be swallowed by `try?`, and then this spun for sixteen seconds
        // under a "Starting…" sweep before falling to "Nothing to preview yet" — the same view
        // you get when nothing was ever asked to start. The one thing the daemon knows and the
        // person does not is *why*, so it is said.
        do {
            _ = try await client.post("/api/dev/start", body: StartDev(command: nil), q(),
                                      as: DevStatus.self)
        } catch {
            lastError = error.localizedDescription
            await refreshDev()
            return
        }
        // The URL appears in the server's own output a moment after it starts.
        for _ in 0..<40 {
            await refreshDev()
            if previewURL != nil { return }
            try? await Task.sleep(for: .milliseconds(400))
        }
        if previewURL == nil {
            lastError = "The dev server started but has not announced a URL yet. Its output is "
                + "in the preview pane."
        }
    }

    // MARK: - Background jobs

    /// Commands Keel is running for this conversation, newest first.
    var monitors: [Wire.Job] = []
    private var monitorTask: Task<Void, Never>?
    /// Whether the background-job loop is up. For the test that asserts closing a lane ends it.
    var isWatchingMonitors: Bool { monitorTask != nil }

    /// Started when the lane is created and never cancelled while it lives.
    ///
    /// Not tied to `running`, which is the whole point: a job outlives the turn that asked for
    /// it, so the loop that notices it finishing has to outlive the turn too. Approvals poll only
    /// while a turn is up because a question cannot exist without one; a job can.
    ///
    /// It used to start on the lane's first *turn*, on the reasoning that a job can only be
    /// created by the agent, so a turn is the only window in which one can appear. That is true
    /// of the job and false of the *panel*: Monitors draws `monitors`, and before any turn had
    /// run in this lane nothing had ever fetched it — so a freshly launched app, or any lane you
    /// open the panel on without typing first, showed "Nothing being watched" as a statement
    /// about the machine when it was a statement about this array never having been filled.
    /// Measured against a live daemon holding a finished job: `reported` was still false, because
    /// nobody had asked. A panel that cannot tell "nothing is running" from "nobody looked" is
    /// the "never weird" failure, and the fix is for somebody to always be looking.
    func watchMonitors() {
        guard monitorTask == nil else { return }
        monitorTask = Task { [weak self, client] in
            while !Task.isCancelled {
                guard let self else { return }
                let q: [String: String] = ["lane": self.id.uuidString]
                if let jobs: [Wire.Job] = try? await client.get("/api/monitors", q) {
                    // Said before the list is replaced, because "new" is what this poll knows and
                    // the next one does not. Nobody is asked whether to monitor a command any
                    // more, so the banner is where "something that outlives this turn just
                    // started" is seen at all.
                    let known = Set(self.monitors.map(\.id))
                    for job in jobs where !known.contains(job.id) && job.running {
                        Notifications.jobStarted(lane: self, command: job.command)
                    }
                    self.monitors = jobs
                    // Acked before it is delivered, and by the one lane that owns it: the result
                    // reaching the conversation twice is worse than not reaching it at all.
                    for job in jobs where !job.running && !job.reported {
                        await self.ack(job)
                        self.deliver(job)
                        // The one people walked away for: a job outlives its turn by design, so
                        // the report landing in a conversation nobody is looking at is not news
                        // reaching anybody.
                        Notifications.jobFinished(lane: self, command: job.command, exit: job.exit)
                    }
                }
                // Two seconds while there is something to watch, fifteen when there is not. The
                // loop is per lane and never ends, so at the flat rate a window with four lanes
                // and no jobs still asked the daemon 172,800 times a day for the same empty list.
                //
                // `running` is in the condition and not just the job list: a job can only be
                // created by the agent, so a turn in flight is the window in which one can
                // appear, and without it a job approved during a slow tick would take fifteen
                // seconds to show up — trading a real delay for traffic nobody was paying for.
                let watching = self.running || self.monitors.contains { $0.running }
                try? await Task.sleep(for: .seconds(watching ? 2 : 15))
            }
        }
    }

    /// The lane is gone from the window. Everything it had running stops.
    ///
    /// `stop()` deliberately leaves `monitorTask` alone — a background job outlives the turn that
    /// asked for it, so the loop that notices it finishing has to outlive the turn too. It does
    /// not have to outlive the *lane*. Until this existed the only thing that ended that loop was
    /// the model being deallocated, which is true eventually and is not a lifetime anyone here
    /// controls: SwiftUI decides when it lets go of a view's model, and "it stops polling at some
    /// point after you close the tab" is not a thing to leave to inference.
    func closed() {
        stop()
        monitorTask?.cancel()
        monitorTask = nil
        treeWatch?.cancel()
        treeWatch = nil
        // A pane nobody is looking at must not keep polling a file every 400 ms.
        unfollow()
    }

    private func ack(_ job: Wire.Job) async {
        struct Id: Encodable { var id: String }
        _ = try? await client.post("/api/monitors/ack", body: Id(id: job.id), as: Bool.self)
    }

    func stopMonitor(_ job: Wire.Job) {
        struct Id: Encodable { var id: String }
        Task { [client] in
            _ = try? await client.post("/api/monitors/stop", body: Id(id: job.id), as: Bool.self)
        }
    }

    /// The prefix a delivered result carries, so the conversation can draw it as a report from
    /// the machine rather than as something the person typed.
    static let jobPrefix = "Background job "

    /// Put a finished job back into the conversation as its own turn.
    ///
    /// Queued rather than dropped when a turn is already up — the same rule as a prompt typed
    /// while the agent works, and for the same reason: the answer is not less wanted because it
    /// arrived at a busy moment.
    private func deliver(_ job: Wire.Job) {
        let took = job.elapsed < 60
            ? "\(Int(job.elapsed))s"
            : "\(Int(job.elapsed) / 60)m\(Int(job.elapsed) % 60)s"
        // Capped: 400 lines of `cargo build` in a chat bubble is not a report, and the agent
        // reads the end of a log rather than the middle of it.
        let tail = job.log.suffix(80).joined(separator: "\n")
        let text = """
            \(Self.jobPrefix)\(job.id) finished — exit \(job.exit ?? -1), after \(took).

            $ \(job.command)

            \(tail.isEmpty ? "(no output)" : tail)
            """
        if running { queued.append(text) } else { start(text) }
    }

    // MARK: - Approvals

    /// Polled only while a turn is running, and only for this conversation.
    private func watchApprovals(_ on: Bool) {
        approvalTask?.cancel()
        approvalTask = nil
        guard on else {
            // Clearing the cards answered nothing. The hook is a process blocked on `/api/approve`
            // and it waits the full four minutes before it gives up, so pressing Stop on a turn
            // with a question on screen left the agent hanging with the screen showing nothing
            // outstanding. Every card gets a decision on the way out.
            let outstanding = pending
            pending = []
            // A plain deny, which the daemon turns into "Not approved. Say what you needed and
            // stop" — the right thing to hear from a turn that is being interrupted. Not an
            // `answer:` string, which is the shape of an AskUserQuestion reply.
            for card in outstanding { answer(card, allow: false, scope: "once") }
            return
        }
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
                // Nothing at all for a minute, heartbeat included, is a dead stream.
                //
                // The daemon sends a keep-alive every fifteen seconds now, so silence is no
                // longer ambiguous: a thinking agent still moves `lastEventAt`, and a daemon that
                // died does not. Before the heartbeat there was nothing to tell those apart, and
                // the stream's own timeout is an hour — so a daemon that went away presented as
                // "thinking…" until somebody gave up. It is a state that could be entered and not
                // left, which is the half of "never stuck" that has no other guard.
                if self.running, Date().timeIntervalSince(self.lastEventAt) > Self.deadStream {
                    let why = "The connection to Keel's daemon stopped responding."
                    self.streamTask?.cancel()
                    self.streamTask = nil
                    self.running = false
                    self.preparing = nil
                    self.watchApprovals(false)
                    self.current?.failure = why
                    self.fail(why, category: "dead-stream",
                              fix: Fix(label: "Retry") { self.retryLast() })
                    Telemetry.track("stream_died", ["mode": self.mode])
                }
                // A turn with output but no *progress* for five minutes is the thing testers
                // describe as "stuck on thinking". Said once per turn, with what it was doing.
                if self.running, !self.stallReported, Date().timeIntervalSince(self.lastProgressAt) > 300 {
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

    /// What `/api/approve/answer` replies. Declared once, beside the call, rather than twice
    /// inside the two `Task` bodies that used it.
    /// (`Wire.Acked` lives with the rest of the wire types.)
    /// Answer a question. The text is what the agent reads as the tool's result.
    func answer(_ p: Wire.Pending, text: String) {
        pending.removeAll { $0.id == p.id }
        Telemetry.track("question_answered")
        let body = Answer(id: p.id, decision: "deny", rules: [], scope: "session",
                          session: p.sessionId.isEmpty ? sessionId : p.sessionId, answer: text)
        Task { [client] in
            do {
                _ = try await client.post("/api/approve/answer", body: body, as: Wire.Acked.self)
            } catch {
                self.answerFailed(p, error)
            }
        }
    }

    /// Approve a plan: answer the tool, switch this lane to Auto, and queue the build.
    ///
    /// Two turns rather than one, because the turn that planned cannot build — it is running under
    /// `--permission-mode plan`, which is the whole reason the plan came back as a question. The
    /// second turn resumes the same conversation, so the plan it is told to implement is the one
    /// directly above it. `send` queues while that first turn finishes and the queue drains when
    /// it ends, by every path that ends it.
    func approvePlan(_ p: Wire.Pending) {
        answer(p, text: Wire.Pending.planApproved)
        mode = "acceptEdits"
        prompt = "Implement the plan you just submitted."
        send()
    }

    func answer(_ p: Wire.Pending, allow: Bool, scope: String) {
        pending.removeAll { $0.id == p.id }
        // The question's own conversation, not this window's guess at it. A card can arrive
        // before `system/init` has landed, and then `sessionId` is nil — which the daemon
        // refuses for a session-scoped rule and used to drop on the floor, so "Allow once, this
        // session" allowed the call and remembered nothing, and the next identical call asked
        // again. `Wire.Pending` has carried the id all along.
        let conversation = p.sessionId.isEmpty ? sessionId : p.sessionId
        let body = Answer(id: p.id, decision: allow ? "allow" : "deny",
                          rules: p.rules, scope: scope, session: conversation)
        Task { [client] in
            do {
                _ = try await client.post("/api/approve/answer", body: body, as: Wire.Acked.self)
            } catch {
                self.answerFailed(p, error)
                return
            }
            // Trusting from an approval changes the window's own claim about itself.
            if scope == "trust" { await self.refreshTrust() }
        }
    }

    /// The card was removed optimistically, which is right — a click should not wait on a round
    /// trip. But if the answer never landed the agent is still blocked, and silently: it waits out
    /// the hook's four-minute timeout while the screen shows an approval that looks answered. So
    /// the question goes back on screen with the reason.
    private func answerFailed(_ p: Wire.Pending, _ error: Error) {
        if !pending.contains(where: { $0.id == p.id }) { pending.append(p) }
        lastError = "That answer did not reach the agent: \(error.localizedDescription). "
            + "It is still waiting — try again."
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
            let a: Adopted = try await client.post("/api/adopt", body: Wire.None(), q(), as: Adopted.self)
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
        // A scan that could not run looks exactly like a scan that found nothing new: the panel
        // keeps what it had and the click reads as a button that does nothing. Say so instead.
        if let why = loadFailed { fail("The scan did not run: " + why, category: "scan") }
        Telemetry.track("rescanned")
    }

    private var reviewKey: String { "keel.lastReview." + repoPath }
    var lastReview: Date? {
        get { UserDefaults.standard.object(forKey: reviewKey) as? Date }
        set { UserDefaults.standard.set(newValue, forKey: reviewKey) }
    }
    /// A review a day: when a project opens into a lane with nothing in it and the last review
    /// is older than a day, it runs on its own. Never into a conversation already in use.
    ///
    /// Behind `Flags.readiness` with the rest of the report. It is a turn Keel starts by itself —
    /// it costs tokens and it opens a lane — and with the Readiness panel hidden there is nowhere
    /// to read what it found or to ask for another. Work nobody can see the result of is the
    /// wrong half of the feature to leave running.
    func offerReview() {
        guard Flags.readiness, !repoPath.isEmpty, loaded else { return }
        if let last = lastReview, Date().timeIntervalSince(last) < 86_400 { return }
        if lanes?.lanes.contains(where: { $0.title == "Staff review" && $0.running }) == true { return }
        Task { await requestReview() }
    }

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
        guard let t: [Wire.Node] = await read("the file tree", "/api/tree", q()) else { return }
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
    /// Open a project, and say so when it does not work.
    ///
    /// Five of the six callers wrote `try? await openProject(…)`, so ⌘O onto a folder that had
    /// moved, a recent project from the palette, and the project menu each did nothing at all —
    /// no error, no change, and the opening bar left up because `opening` is set before the
    /// request and only cleared after it. One reporting wrapper rather than five call sites.
    func open(project path: String) async {
        do {
            try await openProject(path)
        } catch {
            opening = nil
            loaded = true
            lastError = "Could not open \(URL(fileURLWithPath: path).lastPathComponent): "
                + error.localizedDescription
        }
    }

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

    /// The session list only. History polls this while it is open, so a session started in a
    /// terminal appears without a turn having to end here, and "● running" goes away when the
    /// writing stops rather than when something else happens to refresh the whole state.
    func refreshSessions() async {
        guard let fresh: [Wire.Session] = try? await client.get("/api/sessions"),
              fresh != sessions else { return }
        // Only on a change, so a quiet poll invalidates nothing.
        sessions = fresh
        // The list is also the backstop for a followed turn that ended without saying so — ⌃C
        // in the terminal writes no `end_turn`. Claude Code's own pid file does say it.
        if following, let s = fresh.first(where: { $0.id == sessionId }) {
            if s.live != true { following = false }
            if s.busy != true, running, let turn = followed { close(turn) }
        }
    }

    func refreshState() async {
        let s: Wire.State
        do {
            s = try await client.get("/api/state")
        } catch {
            loadFailed = error.localizedDescription
            return
        }
        loadFailed = nil
        loaded = true
        projectOpenKnown = s.projectOpen
        repoPath = s.repo
        // The remembered list is keyed by project, so this is the first moment it can be read —
        // and reading it is the whole point of having written it. `loadCommands` existed, said in
        // its own doc comment that it was there "so the picker works before the first turn", and
        // was called from nowhere: `/` in a freshly opened project offered Keel's own one command
        // and nothing else until a turn had run and the `init` record arrived.
        loadCommands()
        // A project resumed from prefs was never seen by this app's own recents list, so the
        // switcher would be empty on a fresh install until you opened something a second time.
        if s.projectOpen, !s.repo.isEmpty { Recents.remember(s.repo) }
        sessions = s.workspace.sessions
        findings = s.scan.findings
        scan = s.scan
        workspace = s.workspace
        if let policy = s.policy {
            scopeFileLimit = policy.maxFiles
            policySources = policy.sources
            allowedProviders = Set(policy.allowedProviders)
            policyRequiresIsolation = policy.requireIsolation
            isolationByPolicy = policy.isolationByPolicy ?? false
        }
        // The recommendations depend on what is installed, and every path that changes that —
        // install, uninstall, disable, the catalog closing — comes through here. One place,
        // so the badge cannot go stale from a caller that forgot.
        await refreshSuggestions()
    }

    /// Where the transcript read stands. One value, because the pane has to say which of the
    /// three it is: a session still being read, one that could not be read, or one that is
    /// simply empty. This was a Bool and the second case fell into the third — a tail stream
    /// that failed drew "Nothing has run yet" over a conversation that had.
    enum Replay: Equatable { case none, reading, failed(String) }
    var replay: Replay = .none
    /// A transcript is being read into this lane. (`opening` is taken: that one is a project
    /// switch.)
    var replaying: Bool {
        get { replay == .reading }
        set { replay = newValue ? .reading : .none }
    }

    /// The session's transcript is being followed: it is running somewhere Keel does not own it,
    /// and what appears is arriving as it is written.
    var following = false
    /// How much of the conversation was dropped to keep the catch-up bounded.
    var replayDropped = 0
    /// The sentence both panes draw when the daemon dropped the head of a long session. Static so
    /// it can be asserted on; `nil` when nothing was dropped.
    static func droppedNotice(_ dropped: Int) -> String? {
        guard dropped > 0 else { return nil }
        return "\(dropped) earlier record\(dropped == 1 ? " was" : "s were") not loaded — "
            + "this session is longer than Keel replays."
    }
    private var followTask: Task<Void, Never>?
    /// Turns read from the transcript while `replay` is `.reading`, held back until the daemon
    /// says it has caught up, so the pane lays the conversation out once rather than once per
    /// turn. The reading state is the visible one; this is only where the turns wait.
    private var catchUp: [Turn] = []
    /// The turn a followed stream is writing into.
    private var followed: Turn?
    /// Whether the turn on screen is one this lane started.
    ///
    /// A live stream and a followed one carry the same records; what differs is whose turn it is
    /// — and so whether a failure is news or history, whether Keel closes it, and whether Stop
    /// can reach it. Internal rather than private so the parity test can run both paths.
    var owned = false
    private var watchdogTask: Task<Void, Never>?

    func open(session id: String) async {
        sessionId = id
        let known = sessions.first { $0.id == id }
        title = known?.title ?? "Session " + id.prefix(8)
        sessionCwd = known?.elsewhere != nil ? known?.cwd : nil
        // Emptied only once there is something to put back.
        //
        // This used to `turns.removeAll()` here, two awaits before the replacement was ready —
        // and a transcript is the one request that is genuinely slow, hundreds of messages read
        // off disk. Anything that ended the task in between (a second open racing the first, the
        // lane being replaced, a cancelled `.task`) left the pane empty for good, with no turn to
        // draw and no reason on screen for why. Keeping the old conversation up while the new one
        // loads is also the better answer when the read simply fails.
        replaying = true
        replayDropped = 0
        // One path, not two.
        //
        // There were two endpoints here — one for what was said, one for what was done — and a
        // block of code that stitched their answers back into turns. It reconstructed less than
        // the live decoder already produces: no reasoning, no tool arguments (they were faked as
        // `["command": subject]`), no raw lines, tool output cut at 8 KB and the call list at 300.
        // The records on disk are the shape the live decoder reads, so following the file *is*
        // replaying it, and the conversation you reopen is the one you watched.
        //
        // `replaying` is cleared by the stream, when the daemon says it has caught up.
        follow(session: id)
    }

    /// Keep watching the transcript after it has been read.
    ///
    /// This is the half that was missing. Claude Code appends every record to the session's JSONL
    /// as it goes — whoever started it, a terminal or another editor — so the file is already a
    /// live feed, and `open` read it once and stopped. A conversation running elsewhere showed a
    /// snapshot from the moment of the click and then sat still, which reads as Keel being wrong
    /// about its own state rather than as a missing feature.
    ///
    /// Read-only. Composing into a session another process is driving is a claim problem, and
    /// `AppState::claim` is about lanes and working trees rather than conversations — so the
    /// composer says who owns it instead of pretending.
    ///
    /// The daemon sends the same `msg` events the chat stream sends, because the records on disk
    /// are the shape `record` already reads. So a followed turn and a live one are drawn by one
    /// decoder — which is why a reopened conversation now has its reasoning, its tool arguments
    /// and its raw lines, none of which the old two-endpoint replay carried.
    private func follow(session id: String) {
        // Whatever this lane was reading before is over, put back synchronously so the cancelled
        // task's own exit cannot race what starts here.
        unfollow()
        replay = .reading
        owned = false
        running = false
        lastEventAt = Date()
        watchdog()
        followTask = Task { [client] in
            do {
                for try await event in client.events("/api/session/tail",
                                                     self.sq(["id": id, "from": "0"])) {
                    // `break`, never `return`: the lines after this loop are what take the pane
                    // out of "Opening this session…", and a lane closed mid-replay would
                    // otherwise leave a state that can be entered and not left.
                    if Task.isCancelled { break }
                    await self.apply(event)
                }
            } catch {
                // A stream that ends is a session no longer being followed, which is a fact about
                // the pane rather than a failure of the turn — unless it ended before the
                // conversation was on screen, which is a read that failed, and the pane says so.
                if !Task.isCancelled, self.replay == .reading {
                    self.replay = .failed(error.localizedDescription)
                }
            }
            // `unfollow` has already put the state back if this was cancelled.
            if Task.isCancelled { return }
            self.streamEnded()
        }
    }

    /// One consumer for both streams.
    ///
    /// The live chat and the transcript tail emit the same `msg` records, and the two loops that
    /// read them had drifted: a followed turn was never `running`, was never closed, and had no
    /// dead-stream check, so a session watched from a terminal showed no working bar, kept its
    /// last turn "RUNNING" for ever, and sat under "Following this session" after the daemon
    /// died. What differs between the paths is `owned`, and it is one flag here rather than two
    /// loop bodies.
    func apply(_ event: Client.Event) async {
        // The daemon is alive. Nothing to draw — the point is that this moved, which is what
        // tells a thinking agent apart from a stream that has quietly died.
        lastEventAt = Date()
        if event.name == Client.keepAlive { return }
        // Past the heartbeat, so this is the agent itself.
        lastProgressAt = Date()
        switch event.name {
        case "msg":
            guard let data = event.data.data(using: .utf8) else { return }
            if owned {
                preparing = nil
                // `record` keeps the line, because only `record` knows what it is: the raw log
                // is capped at 2,000 lines and partial messages emit one line per token, so
                // keeping every line here filled it with delta noise and dropped every
                // meaningful record behind it.
                if let turn = current { record(data, into: turn) }
                return
            }
            // A person asking opens a turn; everything else belongs to the one open. A
            // transcript that opens with the agent speaking — a resumed or compacted session —
            // still needs somewhere to go.
            if let asked = Self.asked(in: data) {
                adopt(Turn(prompt: asked))
            } else if followed == nil {
                adopt(Turn(prompt: title))
            }
            guard let turn = followed else { return }
            let reading = replay == .reading
            // Past the catch-up this is a session being written right now.
            if !reading { running = true; watchTree(turn) }
            // The transcript's own end of turn: the last assistant record of a turn stops for
            // `end_turn`, and everything after it is the next prompt. Keel was not there to close
            // the turn, so the record does. Measured on this repository's sessions: every turn.
            if record(data, into: turn), !reading { close(turn) }
        case "truncated":
            replayDropped = Int(event.data) ?? 0
        case "caught-up":
            await caughtUp()
        case "err":
            // stderr, as it happens. This is the half that used to escape only on a non-zero
            // exit, so a run that hung reported nothing it had already said.
            current?.note(raw: event.data, stream: .err)
        case "fatal":
            if owned {
                fail(event.data, category: "daemon", fix: repoFix)
                current?.failure = event.data
                if Self.sessionIsGone(event.data) { sessionId = nil }
            } else {
                // The read failed: the transcript is not on this machine, or the daemon could not
                // open it. One surface, in the pane, rather than a banner beside an empty pane.
                replay = .failed(event.data)
            }
        case "starting":
            // The daemon is reading the project. Seconds on a large one, and before this the
            // screen simply stayed empty for all of it.
            preparing = event.data + "…"
        case "done":
            preparing = nil
            // The exit code was sent and thrown away, so a provider that died with nothing on
            // stderr drew a finished turn with nothing in it.
            let code = Int(event.data) ?? 0
            if code != 0, let turn = current, turn.failure == nil {
                let why = "The agent exited with code \(code)."
                turn.failure = why
                fail(why, category: "nonzero_exit", fix: Fix(label: "Retry") { self.retryLast() })
            }
        default:
            // Never discard an event the daemon added. Keel not knowing what something means is
            // not a reason for the person not to see it.
            current?.note(raw: "\(event.name): \(event.data)")
        }
    }

    /// The daemon has read everything that had already happened.
    ///
    /// One assignment, so the conversation is drawn at once: appending each turn as its records
    /// arrived laid the transcript out again for every one of them — six hundred times before a
    /// single finished frame. Records after this are the session running now.
    private func caughtUp() async {
        if replay == .reading { turns = catchUp }
        catchUp = []
        let session = sessions.first { $0.id == sessionId }
        // Only a session something is actually writing to is being *followed*, and only one
        // whose `claude` says it is mid-turn is *running*. Set unconditionally, the first told
        // you a conversation that finished last week was "running outside Keel".
        following = session?.live == true
        let busy = following && session?.busy == true
        for turn in turns {
            turn.settle()
            if !(busy && turn === followed) { turn.finished = true }
        }
        running = busy
        replay = .none
        // The other half of `remember`. The transcript has been read; this puts back what only
        // Keel ever knew about those turns.
        if let id = sessionId { await restoreRecords(session: id) }
        pinTick += 1
        // The tree as it is *now*, not as it was when the project opened. It is also the
        // baseline every followed turn is measured against.
        await refreshGit()
        await refreshTree()
    }

    /// A turn read from a transcript.
    ///
    /// Foreign by construction — Keel was not there when it ran, or is not driving it now — so
    /// nothing here grades it and its failures are history. It used to be marked foreign only
    /// during the catch-up, so a turn that arrived while following was judged as Keel's own:
    /// "GATE · no checks ran" over work Keel never gated, and a red banner for a failure that
    /// happened in somebody's terminal.
    private func adopt(_ turn: Turn) {
        turn.replayed = true
        if let previous = followed {
            // Model-level state is left alone during the catch-up; nothing is on screen yet.
            if replay == .reading { previous.settle(); previous.finished = true } else { close(previous) }
        }
        followed = turn
        if replay == .reading { catchUp.append(turn) } else { turns.append(turn) }
    }

    /// The half of ending a turn that every path shares: what happens whichever way it ended —
    /// done, stopped, closed by its own transcript, or cut off by a dead stream.
    ///
    /// The last deltas arrived inside the coalescing window and there is no next tick to draw
    /// them: a turn that ended would otherwise sit a fraction of a second short of its own final
    /// sentence, permanently. `editing` is cleared here because a followed turn that edited a UI
    /// file used to set it and nothing ever cleared it.
    private func close(_ turn: Turn) {
        turn.settle()
        turn.finished = true
        running = false
        preparing = nil
        editing = nil
        watchApprovals(false)
    }

    /// The tail stream closed on its own: the daemon went away, or the session it was following
    /// did. Whatever was read is better than the spinner it would otherwise be left under, and
    /// the turn that was open is over whether or not its last record said so.
    private func streamEnded() {
        // A read that failed keeps what was on screen; the pane says why, and Retry reads again.
        if replay == .reading, !catchUp.isEmpty {
            catchUp.forEach { $0.settle(); $0.finished = true }
            turns = catchUp
            pinTick += 1
        }
        catchUp = []
        if let turn = followed { close(turn) }
        followed = nil
        if replay == .reading { replay = .none }
        following = false
        watchdogTask?.cancel()
        watchdogTask = nil
    }

    /// A followed stream that goes silent — no records, no heartbeat — for a minute is a dead
    /// daemon, not a quiet agent. The chat stream has always had this check, inside its approval
    /// poll; a followed session polls for nothing, so it had no check at all, and a daemon that
    /// died under it left "Following this session" up for good.
    private func watchdog() {
        watchdogTask?.cancel()
        watchdogTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(15))
                guard let self, !Task.isCancelled else { return }
                self.checkPulse()
            }
        }
    }

    /// One tick of the watchdog, separately so a test can run it without waiting a minute.
    func checkPulse() {
        guard !owned, following || replay == .reading,
              Date().timeIntervalSince(lastEventAt) > Self.deadStream else { return }
        let why = "The connection to Keel's daemon stopped responding."
        followTask?.cancel()
        followTask = nil
        if let turn = followed { close(turn) }
        followed = nil
        catchUp = []
        replay = .failed(why)
        following = false
        Telemetry.track("stream_died", ["mode": "follow"])
    }

    /// The working tree as a set of facts: a path and what git says about it.
    var treeFingerprint: Set<String> { Set(changes.map { $0.path + $0.status }) }

    /// Files that moved in the working tree without naming themselves in a tool call.
    ///
    /// `writeTools` reads the path out of `Edit` and `Write`, and a great deal of real work is
    /// neither: a session in a terminal writes with `cat > file <<'EOF'` as readily as with
    /// `Edit`, and a `Bash` call carries a command, not a path. Measured on the session this was
    /// found in: 54 tool calls, every one of them `Bash`, several of them writing whole files —
    /// so the turn reported no files changed while the diff beside it was full of them.
    ///
    /// git does not care which tool did the writing, so it is the evidence rather than the
    /// arguments. Attributed against a fingerprint taken before, so this names what *this* turn
    /// moved rather than everything uncommitted in the checkout.
    /// Through `repoRelative`, because `Edit` names a file absolutely and git names it from the
    /// root of the checkout: added blind, a file the turn wrote *and* dirtied would be two rows
    /// for one file, spelt differently.
    func attribute(_ before: Set<String>, to turn: Turn) {
        for change in changes where !before.contains(change.path + change.status) {
            guard !turn.files.contains(where: { repoRelative($0) == change.path }) else { continue }
            turn.noteEdit(change.path)
        }
    }

    // MARK: - What only Keel knows about a turn

    /// Persist a finished turn's Keel-side facts, so the next launch can put them back.
    ///
    /// Claude Code's transcript is the record of the conversation and nothing more: what was said,
    /// which tools ran, with what arguments. Everything the turn pane draws around that is Keel's
    /// own — the files `attribute` read out of git, the shell writes `Turn.written` proved by
    /// mtime, the commit, the gate, the duration, the cost. All of it was computed here and kept
    /// nowhere, so a relaunch rebuilt the turn from the transcript and it came back with its prose
    /// and none of its work. For a turn whose calls are all `Bash` — a heredoc, a `tee`, a
    /// formatter — that is every file it touched.
    ///
    /// Failure is silent on purpose. This is a sidecar: a turn that cannot be written down is
    /// still a turn that happened, and an error banner about a cache would be noise about
    /// something the person did not ask for.
    func remember(_ turn: Turn) async {
        guard let session = sessionId, let n = turns.firstIndex(where: { $0 === turn }) else { return }
        let body = Wire.TurnRecordRequest(session: session, record: record(of: turn, n: n))
        _ = try? await client.post("/api/turns", body: body, q(), as: Wire.TurnRecord.self)
    }

    /// A turn as the sidecar stores it.
    private func record(of turn: Turn, n: Int) -> Wire.TurnRecord {
        var out = Wire.TurnRecord(n: n)
        out.prompt = String(turn.prompt.prefix(200))
        out.files = turn.files
        out.commit = turn.commit
        out.ms = turn.durationMS
        out.cost = turn.cost
        if let t = turn.tokens {
            out.tokens = .init(input: t.input, output: t.output,
                               cache_read: t.cacheRead, cache_write: t.cacheWrite)
        }
        switch turn.gate {
        case .notRun: out.gate = nil
        case .running(let c): out.gate = .init(status: "running", command: c)
        case .passed(let c, _): out.gate = .init(status: "passed", command: c)
        case .none(let c): out.gate = .init(status: "none", command: c)
        case .failed(let c, let problems):
            out.gate = .init(status: "failed", command: c,
                             problems: problems.map { "\($0.file):\($0.line) \($0.message)" })
        }
        return out
    }

    /// Put back what the transcript could not carry, once the replay has caught up.
    ///
    /// Matched by turn number and checked against the prompt, because a transcript is not a stable
    /// index: it can be compacted, resumed, or opened from a different starting point, and a record
    /// landing quietly on the wrong turn would report one turn's files against another. That is a
    /// worse failure than the one this fixes — a wrong answer beats no answer nowhere in this
    /// product — so a mismatch drops the record and leaves the turn as the transcript had it.
    ///
    /// Never overwrites. A followed turn's live values are the ones actually measured just now;
    /// this only fills what the replay left empty.
    private func restoreRecords(session id: String) async {
        let stored: [Wire.TurnRecord]
        do {
            stored = try await client.get("/api/turns", q(["session": id]))
        } catch {
            return
        }
        for record in stored {
            guard turns.indices.contains(record.n) else { continue }
            let turn = turns[record.n]
            guard record.prompt.isEmpty || turn.prompt.hasPrefix(record.prompt)
                    || record.prompt.hasPrefix(String(turn.prompt.prefix(200)))
            else { continue }

            for path in record.files { turn.noteEdit(path) }
            if turn.commit == nil { turn.commit = record.commit }
            if turn.durationMS == nil { turn.durationMS = record.ms }
            if turn.cost == nil { turn.cost = record.cost }
            if turn.tokens == nil, let t = record.tokens {
                turn.tokens = .init(input: t.input, output: t.output,
                                    cacheRead: t.cache_read, cacheWrite: t.cache_write)
            }
            if case .notRun = turn.gate, let gate = record.gate {
                switch gate.status {
                case "passed": turn.gate = .passed(gate.command ?? "", 0)
                case "none": turn.gate = .none(gate.command ?? "")
                // A gate that was still running when Keel was last quit did not finish, and
                // redrawing it as a spinner would be a turn that can be entered and not left.
                case "failed", "running": turn.gate = .failed(gate.command ?? "", [])
                default: break
                }
            }
        }
    }

    /// Read the working tree again because a session Keel is not driving just wrote to it.
    ///
    /// Nothing else does: `endTurn` is what refreshes the panels, and a followed turn never
    /// reaches it — so the Changes panel kept whatever it read when the project opened, for as
    /// long as the session ran.
    ///
    /// Coalesced, because a turn writes several files in a row and each one would otherwise be a
    /// `git status` nobody is waiting for.
    private var treeWatch: Task<Void, Never>?

    func watchTree(_ turn: Turn) {
        treeWatch?.cancel()
        treeWatch = Task {
            try? await Task.sleep(for: .milliseconds(600))
            guard !Task.isCancelled else { return }
            let before = treeFingerprint
            await refreshGit()
            await refreshTree()
            attribute(before, to: turn)
            // A followed turn never reaches `endTurn`, so this was the only place its files were
            // ever known — and they were kept nowhere. The next replay rebuilt the turn from the
            // transcript, which names `Edit` and `Write` and nothing a heredoc wrote, and the
            // list went blank. Upserted, so a burst of writes is one record, not many.
            await remember(turn)
            diffTick += 1
        }
    }

    /// A transcript timestamp, in both the shapes the CLI writes it.
    ///
    /// With and without fractional seconds: a miss on either is silent — the footer simply does
    /// not draw, which is the state a session opened from History was already in.
    nonisolated static func moment(_ text: String) -> Date? {
        let full = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
        let plain = Date.ISO8601FormatStyle()
        return (try? full.parse(text)) ?? (try? plain.parse(text))
    }

    /// The prompt a transcript record carries, when it is a person asking rather than a tool
    /// answering. A `user` record holding `tool_result` blocks is the second kind and belongs to
    /// the turn already open.
    static func asked(in data: Data) -> String? {
        guard let r = try? JSONDecoder().decode(Record.self, from: data), r.type == "user" else {
            return nil
        }
        if let blocks = r.message?.content {
            if blocks.contains(where: { $0.type == "tool_result" }) { return nil }
            let said = blocks.filter { $0.type == "text" }.compactMap(\.text).joined(separator: "\n")
            return said.isEmpty ? nil : said
        }
        return nil
    }

    /// Whether this project is trusted, which the window says out loud for as long as it is true.
    ///
    /// A permission granted once and then forgotten is the one that surprises you later, so it is
    /// not enough for this to be correct — it has to be visible.
    var trusted = false

    struct PermissionsView: Decodable { var trusted: Bool }

    struct PathBody: Encodable { var path: String }
    struct RenameBody: Encodable { var path: String; var name: String }
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
        props["refused"] = refusedThisTurn.count
        Telemetry.track("turn_finished", props)
        // The shape of every "Keel keeps rejecting me" report: a tool was refused and no card
        // was ever put in front of anyone, so there was nothing to click and no reason given.
        // It has had several causes — a hook that could not parse its own arguments, a session
        // resumed in the wrong project, a lane that could not read the project root — and it is
        // the *symptom* that is worth watching, because it is the same however it is reached.
        if !refusedThisTurn.isEmpty, approvalsThisTurn == 0 {
            Telemetry.warn("refused without asking", [
                "tools": Set(refusedThisTurn).sorted().joined(separator: ","),
                "count": "\(refusedThisTurn.count)",
                "mode": mode,
                "isolated": isolated ? "yes" : "no",
                "resumed": sessionId == nil ? "no" : "yes",
            ])
        }
        // A turn that failed the gate or ran no tools is the shape of a bad experience; a
        // warning so it is findable next to the crashes rather than buried in a funnel.
        if case .failed = turn.gate {
            Telemetry.warn("turn failed the gate", ["mode": mode, "calls": "\(turn.calls.count)"])
        }
        approvalsThisTurn = 0
        refusedThisTurn = []
    }
    var approvalsThisTurn = 0

    /// The tools refused this turn. Names only — never the path or command that was refused.
    var refusedThisTurn: [String] = []

    /// Claude Code's own wording when a tool is denied, confirmed against real transcripts rather
    /// than guessed: "Claude requested permissions to read from …, but you haven't granted it
    /// yet." The tail is the stable half; the verb and the subject both vary.
    private func noteRefusal(in output: String, call id: String, turn: Turn) {
        guard output.contains("haven't granted it yet") else { return }
        refusedThisTurn.append(turn.toolName(of: id) ?? "unknown")
    }

    /// A read that failed says so.
    ///
    /// `try?` on these was the quiet half of every "blank pane" report: the Changes panel and the
    /// file tree simply kept whatever they last had, with nothing on screen saying the daemon had
    /// stopped answering. Unlike `attempt`, success clears nothing — a turn's own error must not
    /// be erased by the next `git status` that happens to work.
    private func read<T: Decodable & Sendable>(_ what: String, _ path: String,
                                    _ q: [String: String] = [:]) async -> T? {
        do { return try await client.get(path, q) } catch {
            lastError = "Could not read \(what): \(error.localizedDescription)"
            let e = error as NSError
            Telemetry.warn("read failed", ["what": what, "domain": e.domain, "code": "\(e.code)"])
            return nil
        }
    }

    private func attempt(_ work: () async throws -> Void) async {
        do { try await work(); lastError = nil; lastFix = nil } catch {
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

    /// The daemon's field is `name`. It was sent as `title`, which axum rejected with a 422 that
    /// `try?` swallowed — so renaming a tab changed the tab and nothing else, and the session kept
    /// its old name in History where people then went looking for it. Pinned by a test.
    struct SessionRename: Encodable { var id: String; var name: String }

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
        /// What the daemon answers with. It was decoded as `Bool`, which never matched either —
        /// two swallowed mismatches on one call is how a rename came to fail in silence.
        struct Renamed: Decodable { var name: String }
        do {
            _ = try await client.post("/api/session/rename",
                                      body: SessionRename(id: id, name: title), as: Renamed.self)
        } catch {
            lastError = "Renamed the tab, but History kept the old name: "
                + error.localizedDescription
        }
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

    /// Bumped when the working tree changed under a diff someone is looking at: a stage, a
    /// discard, a rewind, and the end of every turn.
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
    /// Every repository in the opened folder. One for an ordinary project; several when the
    /// folder is a workspace holding `backend/` and `frontend/`.
    var repos: [Wire.Repo] = []
    /// The folder is a workspace of repositories rather than one project.
    var isWorkspace: Bool { repos.count > 1 || (repos.count == 1 && !repos[0].dir.isEmpty) }
    /// `changes` is a folded, capped view of a working tree with thousands of files in it.
    var changesCollapsed = false

    func refreshGit() async {
        if let s: Wire.GitStatus = await read("git status", "/api/git/status", q()) {
            isRepo = s.isRepo
            branch = s.branch
            changes = s.changes
            repos = s.repos
            changesCollapsed = s.collapsed
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

    /// The past session being renamed from History, and the name being typed for it.
    ///
    /// On the model for the same reason as `confirmingDiscard`, and it took the same bug to
    /// learn it twice: the alert used to hang off a zero-height `Color.clear` at the end of the
    /// session list, which a lazy stack with 158 rows in it never builds. Right-click, Rename,
    /// and nothing at all happened.
    var renamingSession: String?
    var renameDraft = ""
    var discarded: String?
    func discardAll() async {
        var counts = [0, 0]
        await git("discard") { counts = try await client.post("/api/git/discard-all", body: Wire.None(), q(), as: [Int].self) }
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
            _ = try await client.post("/api/git/push", body: Wire.None(), q(), as: String.self)
            lastError = nil; lastFix = nil
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

    struct CommitBody: Encodable { var message: String; var automatic = false }

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
        // A pin whose pixels did not move is the failure the Designer exists to catch. Committing
        // it anyway would bury the one turn worth looking at under a commit that says it passed.
        if turn.design?.pins.contains(where: { $0.verdict == .nothingChanged }) == true { return }
        let first = turn.prompt.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .first { !$0.isEmpty && !$0.hasPrefix("@") } ?? "agent turn"
        let subject = String(first.prefix(72))
        let body = "Made by the agent in Keel, lane \"\(title)\". The project's checks "
            + (turn.gate == .notRun ? "were not run." : "passed.")
        do {
            let committed = try await client.post("/api/git/commit",
                                                  body: CommitBody(message: subject + "\n\n" + body,
                                                                   automatic: true),
                                                  q(), as: Bool.self)
            await refreshGit()
            // Only a commit that happened is this turn's; otherwise the footer would show the
            // previous one as if it were new.
            if committed { turn.commit = commits.first?.sha }
        } catch {
            lastError = "Could not commit: " + error.localizedDescription
        }
    }

    /// Take the last commit apart, keeping its changes. `--soft` on the daemon's side.
    func uncommit() async {
        await attempt { _ = try await client.post("/api/git/uncommit", body: Wire.None(), q(), as: Bool.self) }
        await refreshGit()
    }

    func gitInit() async -> String? {
        do {
            _ = try await client.post("/api/git/init", body: Wire.None(), q(), as: Bool.self)
            await refreshGit()
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    /// The diff, or the reason there isn't one.
    ///
    /// This was `try?`, and the pane read `nil` as "this file matches HEAD — the change was
    /// committed or undone". A daemon that is down, a checkout removed outside Keel and a refused
    /// `?wt=` all arrived as that sentence: not a blank pane, which would at least look like a
    /// failure, but a confident and wrong claim about the person's code.
    func diff(_ path: String) async -> Result<Wire.Diff, Error> {
        do {
            return .success(try await client.get("/api/git/diff", q(["path": path])))
        } catch {
            return .failure(error)
        }
    }
}
