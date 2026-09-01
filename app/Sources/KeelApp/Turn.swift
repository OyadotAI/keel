import AppKit
import Foundation

/// One turn of agent work, which is the unit this whole application is built around.
///
/// A turn is not a chat message. It is: what you asked, every file the agent wrote, every command
/// it ran and what that command printed, whether the project's own gate passed afterwards, how
/// long it took and what it cost. All of that already exists — it was just scattered across four
/// panels. Here it is one object, so it can be one view.
@MainActor
@Observable
final class Turn: Identifiable {
    let id = UUID()
    let prompt: String
    let started = Date()

    /// Prose from the agent, accumulated from `text_delta`s.
    ///
    /// The whole turn's prose as one string, which is what every consumer outside the conversation
    /// wants — the copy buttons, the reports, the review packet. What it cannot say is *when* each
    /// part of it was said; `steps` is the ordered form.
    var text = ""
    /// Whether any of it arrived as deltas; when none did, the whole `assistant` message is
    /// the only copy and is taken instead.
    var streamedText = false
    var thinking = ""

    // MARK: - The turn in order

    /// One streamed content block.
    ///
    /// A class, so a delta mutates *this* object rather than the turn: under Observation that
    /// invalidates the one row being written and nothing else. Appending to `Turn.text` invalidated
    /// every view that read the turn, which was both panes and every finished row in them.
    @MainActor
    @Observable
    final class Block: Identifiable {
        let id = UUID()

        /// The text as it stands. **Not observed**, deliberately.
        ///
        /// A token arrives faster than a frame, and a view that redraws on the raw string redraws
        /// once per token for a picture that cannot change more than sixty times a second. Only
        /// `blocks` is observed, and `blocks` moves on a clock.
        @ObservationIgnored private(set) var text = ""

        /// The parse the views render from.
        private(set) var blocks: [Markdown.Block] = []
        /// How many words it holds, for a label on something collapsed. Counted in `settle`
        /// rather than read off `text` in a view: `text` is not observed, so a label reading it
        /// would go stale — and counting words walks the whole string, once per render.
        private(set) var words = 0
        /// What `blocks` was parsed from, so a flush with nothing new is free.
        @ObservationIgnored private var parsed = 0
        @ObservationIgnored private var flushing = false

        init(_ text: String = "") { append(text); settle() }

        /// Roughly twelve times a second — under a frame, far above what anyone reads at.
        ///
        /// The parse is O(n) in the whole block, so running it per token is O(n²) in the reply.
        /// That is the shape of the bug this class was written to remove, and re-parsing "only
        /// the current block" instead of the whole turn was a smaller constant on the same curve:
        /// the current block *is* most of a reply. The cure is the clock, not the scope.
        static let coalesce = Duration.milliseconds(80)

        func append(_ more: String) {
            guard !more.isEmpty else { return }
            text += more
            guard !flushing else { return }
            flushing = true
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: Block.coalesce)
                self?.flushing = false
                self?.settle()
            }
        }

        /// Parse now: the block closed, or the turn ended, and there is no next tick to correct a
        /// half-drawn reply.
        func settle() {
            guard text.count != parsed else { return }
            parsed = text.count
            blocks = Markdown.blocks(text)
            words = text.split(whereSeparator: \.isWhitespace).count
        }
    }

    /// What the turn did, in the order it did it.
    ///
    /// The CLI interleaves prose, reasoning and tool calls; a turn that says "I'll check the router
    /// first", greps, and then explains what it found is three things in a sequence. Keel kept the
    /// prose in one string and the calls in a list beside it, so the sequence was unrecoverable —
    /// which is most of what "it hides what the CLI shows" means.
    ///
    /// A call is referenced by id rather than held: `calls` stays the owner, so grouping, risk and
    /// the trace pane are unchanged.
    enum Step: Identifiable {
        case say(Block)
        case think(Block)
        case call(String)

        var id: String {
            switch self {
            case .say(let b): "say-\(b.id)"
            case .think(let b): "think-\(b.id)"
            case .call(let id): "call-\(id)"
            }
        }
    }

    private(set) var steps: [Step] = []

    /// The block currently being written, when it is of this kind. A delta belongs to the block
    /// that was last opened; a `content_block_start` opens a new one.
    private var open: Step?

    /// Content-block index → the tool call it announced, for the span between that block's
    /// `content_block_start` and its `content_block_stop`. Indices restart with every message, so
    /// this lives on the turn and is emptied as each block closes.
    var openBlocks: [Int: String] = [:]

    /// Open a new prose block. Called on `content_block_start` for a `text` block.
    func say() { let b = Block(); steps.append(.say(b)); open = .say(b) }
    /// Open a new reasoning block.
    func think() { let b = Block(); steps.append(.think(b)); open = .think(b) }

    /// Prose, into the open block — opening one first if the stream never announced a block start.
    func said(_ more: String) {
        if case .say(let b)? = open { b.append(more) } else { say(); said(more); return }
        text += more
        streamedText = true
    }

    /// A whole block that arrived at once, from a run with no partial messages.
    ///
    /// Deliberately not `say()` + `said()`: `said` sets `streamedText`, which is the flag telling
    /// the next complete message that its prose has already been seen. Taking that path here would
    /// mean the first whole message rendered and every one after it was dropped.
    func wrote(_ whole: String) {
        guard !whole.isEmpty else { return }
        steps.append(.say(Block(whole)))
        open = nil
        if !text.isEmpty { text += "\n\n" }
        text += whole
    }

    /// Parse whatever is still pending, now.
    ///
    /// Blocks parse on a clock while a reply streams, which is right — a token arrives faster
    /// than a frame. A turn that has ended has no next tick to correct a half-drawn block.
    func settle() { steps.forEach { if case .say(let b) = $0 { b.settle() }
                                    else if case .think(let b) = $0 { b.settle() } } }

    /// A record was written at this moment.
    ///
    /// A live turn measures itself — `duration_ms` off the `result`. A transcript has no `result`
    /// record at all, so a replayed turn's elapsed time is the span of its own records. Only ever
    /// widens, and only when the two ends are genuinely apart: a turn with one record reporting
    /// `0s` is a measurement nobody made.
    func saw(_ at: Date) {
        if firstRecord == nil { firstRecord = at }
        guard let from = firstRecord, at > from else { return }
        if durationMS == nil || Int(at.timeIntervalSince(from) * 1000) > durationMS! {
            durationMS = Int(at.timeIntervalSince(from) * 1000)
        }
    }
    private var firstRecord: Date?

    /// A whole reasoning block that arrived at once — a transcript holds them complete.
    func mused(_ whole: String) {
        guard !whole.isEmpty else { return }
        steps.append(.think(Block(whole)))
        open = nil
        if !thinking.isEmpty { thinking += "\n\n" }
        thinking += whole
    }

    /// Reasoning, into the open block.
    func thought(_ more: String) {
        if case .think(let b)? = open { b.append(more) } else { think(); thought(more); return }
        thinking += more
    }

    /// Files written, in the order first touched. Order matters: it is the shape of the work.
    private(set) var files: [String] = []
    /// Tool calls in order, keyed for pairing with the result that answers them.
    private(set) var calls: [Call] = []
    private var callIndex: [String: Int] = [:]
    /// A subagent's call, by id, to the top-level call it belongs under.
    private var parentOf: [String: String] = [:]

    /// The daemon caps a replay at 300 calls and says so; hiding that would be a quiet lie about
    /// what the session did.
    var truncated = false
    var cost: Double?
    var durationMS: Int?
    var finished = false
    /// The commit Keel made of this turn's work, once the gate passed.
    var commit: String?

    /// What the turn spent, from the CLI's own `result`. Cache tokens are counted separately
    /// because they are what makes the cost figure make sense: a turn with 90% cache reads is
    /// cheap in a way its input count alone hides.
    var tokens: Tokens?
    /// The last request's input — how much of the context window the conversation now holds.
    var contextTokens = 0

    struct Tokens: Equatable {
        var input = 0
        var output = 0
        var cacheRead = 0
        var cacheWrite = 0

        var total: Int { input + output + cacheRead + cacheWrite }
        /// The share of what was read that came from cache.
        var cached: Double {
            let read = input + cacheRead + cacheWrite
            return read > 0 ? Double(cacheRead) / Double(read) : 0
        }
        static func + (a: Tokens, b: Tokens) -> Tokens {
            Tokens(input: a.input + b.input, output: a.output + b.output,
                   cacheRead: a.cacheRead + b.cacheRead, cacheWrite: a.cacheWrite + b.cacheWrite)
        }
    }

    /// The working tree before this turn ran, as a git tree id. `nil` for a replayed turn, or
    /// when the snapshot could not be taken.
    var snapshot: String?

    /// The provider reported the run itself as failed, whether or not it said why.
    var failed = false

    /// Everything the provider actually emitted, in order: stdout, stderr, and the exit line.
    ///
    /// The guarantee this exists for is that there is no state in which Keel saw something and the
    /// person cannot. Capped, because `turns` is never trimmed in memory and a long session would
    /// otherwise grow without bound.
    private(set) var raw: [RawLine] = []
    private(set) var rawDropped = 0

    struct RawLine: Identifiable {
        let id = UUID()
        var text: String
        var stream: Stream = .out
        enum Stream { case out, err }
    }

    static let rawCap = 2000

    /// Lines that arrived and could not be decoded, and record types Keel had no case for. Both
    /// used to be silent; both are now reasons the raw view exists.
    var unreadable = 0
    var unknown: Set<String> = []

    func note(raw line: String, stream: RawLine.Stream = .out) {
        guard raw.count < Self.rawCap else { rawDropped += 1; return }
        raw.append(RawLine(text: line, stream: stream))
    }

    /// What Claude Code said went wrong, when it reported a failure about itself rather than
    /// about the work. Kept per turn so the next send does not erase it.
    var failure: String?

    /// This turn was meant to run in an isolated checkout and could not, with the reason.
    ///
    /// It ran in the project instead. Refusing was worse: a customer whose `git worktree add`
    /// failed could not make progress at all, and isolation is Keel's own default rather than
    /// something they asked for. Where a policy file demands isolation, the turn still refuses.
    var notIsolated: String?

    /// True for a turn rebuilt from a transcript rather than watched live.
    ///
    /// A replayed turn has no gate result and never will — Keel was not there when it ran. Saying
    /// "NO CHECKS" about it is inventing a finding, and eight of those down a column is noise that
    /// buries the one turn that did fail.
    var replayed = false

    /// Whether this turn did anything to the repository. A turn that only answered a question has
    /// nothing to show in a record of what changed.
    var didWork: Bool { !files.isEmpty || !calls.isEmpty }

    /// Set when the turn started from a click in the preview: what it looked like before and
    /// after, and whether the pixels agree that something happened.
    var design: Design?

    struct Design {
        /// One per pin the turn was sent with. It was one element for a long time and the check
        /// silently only ever ran on the first: every other pin paid for a before-image nobody
        /// ever compared it to.
        var pins: [Pin] = []
        var duplicated = false
        /// What the page said moved, and a photograph of all of it together.
        var regions: [Region] = []
        var pageAfter: NSImage?

        struct Pin: Identifiable {
            let id = UUID()
            var selector: String
            var before: NSImage?
            var after: NSImage?
            var verdict: DesignCheck.Verdict
        }
    }

    /// The project's own checks, run after the turn — not the agent's opinion of its own work.
    var gate: Gate = .notRun

    enum Gate: Equatable {
        case notRun
        case running(String)
        case passed(String, TimeInterval)
        case failed(String, [Wire.Problem])
        case none(String)
    }

    struct Call: Identifiable {
        let id: String
        var tool: String
        /// The one-line subject: a command, a path, a pattern — whatever the tool was actually about.
        var subject: String
        /// Everything the agent passed the tool.
        ///
        /// Only `subject` used to survive — one field, first line, 160 characters — so an `Edit`
        /// showed a path and never its hunks, and a heredoc showed the word `cat`. The CLI prints
        /// all of it, and the whole complaint about Keel hiding raw data starts here.
        var input: [String: JSONValue] = [:]
        /// `input_json_delta` as it arrives, before the block closes and it can be parsed. This is
        /// what lets a command appear while it is being written rather than only once it is run.
        var partialInput = ""
        var reason: String?
        var output: String = ""
        var failed = false
        var running = true
        /// When it started and — once it has — when it stopped. A list of commands that cannot say
        /// how long one took cannot tell you whether the one you are watching is slow or stuck.
        let started = Date()
        var ended: Date?
        /// Calls a subagent made on this call's behalf. Only a `Task` has any.
        var children: [Call] = []

        var duration: TimeInterval? { ended.map { $0.timeIntervalSince(started) } }

        /// How much this call could change, read off its own text.
        ///
        /// Display only, and it must stay that way: the decision that actually gates a command is
        /// the approval hook, which parses properly in Rust and never consults this. What this
        /// buys is that `rm -rf` and `ls` are not the same grey dot in a list of a hundred rows.
        var risk: Risk {
            if Turn.writeTools.contains(tool) { return .warn }
            guard tool == "Bash" else { return .safe }
            return Turn.risk(of: subject)
        }

        /// The call doing something right now, however deep.
        var deepestRunning: Call? {
            guard running else { return nil }
            return children.last(where: \.running)?.deepestRunning ?? self
        }
    }

    init(prompt: String) { self.prompt = prompt }

    /// Tools whose use means a file changed. Mirrors `WRITE_TOOLS` in the web UI, deliberately:
    /// the two must agree about what "the agent edited something" means or the two renderings of
    /// the same turn disagree.
    nonisolated static let writeTools: Set<String> = ["Edit", "Write", "MultiEdit", "NotebookEdit", "Update"]

    func noteEdit(_ path: String?) {
        guard let path, !path.isEmpty, !files.contains(path) else { return }
        files.append(path)
    }

    /// `parent` is the `Task` call a subagent is working for. Its calls nest under that row
    /// rather than joining the top-level list: a subagent that reads forty files is one line
    /// saying so, and forty lines is the transcript nobody could follow in the terminal either.
    /// The one-line summary of a call, in the order the tools actually carry it.
    static func subject(of input: [String: JSONValue]) -> String {
        let subject = ["command", "file_path", "path", "pattern", "description"]
            .compactMap { input[$0]?.stringValue }
            .first ?? ""
        let oneLine = subject.split(separator: "\n").first.map(String.init) ?? ""
        return String(oneLine.prefix(160))
    }

    /// Idempotent in `id`.
    ///
    /// A call is announced twice when partial messages are on: once as a `content_block_start`,
    /// which is what lets it appear while its arguments are still being written, and again in the
    /// complete `assistant` message. The second is the repair, not a second call.
    func begin(call id: String, tool: String, input: [String: JSONValue], parent: String? = nil) {
        if let existing = index(of: id) {
            // The complete message arriving after the stream. Its input is the authoritative one.
            if !input.isEmpty { update(call: existing, input: input) }
            return
        }
        let call = Call(id: id, tool: tool, subject: Self.subject(of: input), input: input,
                        reason: input["description"]?.stringValue)
        if let parent, let pi = callIndex[parent] {
            parentOf[id] = parent
            calls[pi].children.append(call)
            // A `Group` holds copies of its calls, so a child appended here is invisible to the
            // rows until the parent's copy is refreshed. Without this the nested view — the whole
            // point of a `Task` row — stayed empty for the entire run of the subagent and then
            // filled in at once when the `Task` itself returned.
            regroup(pi)
        } else {
            callIndex[id] = calls.count
            calls.append(call)
            steps.append(.call(id))
            open = nil
            group(call)
        }
        // A file a subagent wrote is still a file this turn wrote.
        if Self.writeTools.contains(tool) {
            noteEdit(input["file_path"]?.stringValue ?? input["path"]?.stringValue)
        }
    }

    /// Where a call lives: a top-level index, or a parent index and a child index.
    private enum Where { case top(Int); case child(Int, Int) }

    private func index(of id: String) -> Where? {
        if let i = callIndex[id] { return .top(i) }
        if let parent = parentOf[id], let pi = callIndex[parent],
           let ci = calls[pi].children.firstIndex(where: { $0.id == id }) {
            return .child(pi, ci)
        }
        return nil
    }

    private func update(call: Where, input: [String: JSONValue]) {
        let subject = Self.subject(of: input)
        switch call {
        case .top(let i):
            calls[i].input = input
            calls[i].subject = subject
            calls[i].reason = input["description"]?.stringValue ?? calls[i].reason
            regroup(i)
        case .child(let pi, let ci):
            calls[pi].children[ci].input = input
            calls[pi].children[ci].subject = subject
            regroup(pi)
        }
        if Self.writeTools.contains(tool(at: call)) {
            noteEdit(input["file_path"]?.stringValue ?? input["path"]?.stringValue)
        }
    }

    private func tool(at call: Where) -> String {
        switch call {
        case .top(let i): calls[i].tool
        case .child(let pi, let ci): calls[pi].children[ci].tool
        }
    }

    /// Arguments arriving a fragment at a time, before the block closes.
    func argue(call id: String, json: String) {
        switch index(of: id) {
        case .top(let i)?: calls[i].partialInput += json
        case .child(let pi, let ci)?: calls[pi].children[ci].partialInput += json
        case nil: break
        }
    }

    /// The block closed: the accumulated fragments are now a complete JSON object.
    func settle(call id: String) {
        guard let at = index(of: id) else { return }
        let partial = switch at {
        case .top(let i): calls[i].partialInput
        case .child(let pi, let ci): calls[pi].children[ci].partialInput
        }
        guard !partial.isEmpty,
              let input = try? JSONDecoder().decode([String: JSONValue].self,
                                                    from: Data(partial.utf8))
        else { return }
        update(call: at, input: input)
    }

    /// The call a `Step.call` refers to. Top level only — a subagent's calls are drawn nested
    /// under the `Task` that started them, not as steps of their own.
    func call(_ id: String) -> Call? { callIndex[id].map { calls[$0] } }

    /// Which tool a call id belongs to, including one made by a subagent.
    func toolName(of id: String) -> String? {
        if let parent = parentOf[id], let pi = callIndex[parent] {
            return calls[pi].children.first { $0.id == id }?.tool
        }
        return callIndex[id].map { calls[$0].tool }
    }

    func finish(call id: String, output: String, failed: Bool) {
        switch index(of: id) {
        case .child(let pi, let ci)?:
            calls[pi].children[ci].output = output
            calls[pi].children[ci].failed = failed
            calls[pi].children[ci].running = false
            calls[pi].children[ci].ended = Date()
            regroup(pi)
        case .top(let i)?:
            calls[i].output = output
            calls[i].failed = failed
            calls[i].running = false
            calls[i].ended = Date()
            regroup(i)
        case nil:
            break
        }
    }

    // MARK: - Risk

    /// What a call could do, in three tiers.
    enum Risk: Int, Comparable {
        /// Reads something and changes nothing.
        case safe
        /// Writes, installs, commits, or runs something Keel cannot vouch for. The default for an
        /// unrecognised program: "we do not know this reads only" is the honest answer, and
        /// claiming safety for an unknown binary is the one mistake this must not make.
        case warn
        /// Destroys or publishes. Irreversible, or reversible only by someone who notices.
        case danger

        static func < (a: Risk, b: Risk) -> Bool { a.rawValue < b.rawValue }
    }

    /// Programs that only read. Everything else is at least `warn`.
    private nonisolated static let readers: Set<String> = [
        "ls", "cat", "head", "tail", "wc", "grep", "rg", "egrep", "find", "fd", "echo", "pwd",
        "which", "file", "stat", "du", "df", "ps", "date", "tree", "jq", "yq", "diff", "sort",
        "uniq", "cut", "tr", "basename", "dirname", "printf", "sed", "awk", "less", "column",
        "nl", "seq", "true", "type", "id", "whoami", "hostname", "uname", "man", "open",
    ]

    /// `git` and friends are read or write depending on the next word alone.
    private nonisolated static let readSubcommands: Set<String> = [
        "git status", "git diff", "git log", "git show", "git branch", "git blame", "git ls-files",
        "git rev-parse", "git remote", "git config", "git describe", "git stash list",
        "cargo tree", "cargo metadata", "npm ls", "brew list", "docker ps", "docker images",
        "kubectl get", "kubectl describe", "gh pr view", "gh run list",
    ]

    /// Irreversible, or reversible only by whoever notices. Matched as substrings because what
    /// makes them dangerous is usually a flag, not the program.
    private nonisolated static let dangerous: [String] = [
        "rm -r", "rm -f", "rm *", "sudo ", "doas ", "dd if=", "mkfs", "shutdown", "reboot",
        "killall", "chmod 777", "chmod -r 777", "git push --force", "git push -f", "git reset --hard",
        "git clean -", "git checkout -- .", "branch -d", "sed -i", "npm publish", "cargo publish",
        "docker system prune", "> /dev/sd", "curl -fssl", "| sh", "| bash", "drop table",
        "truncate ", "shred ", "history -c", "wrangler delete", "kubectl delete",
    ]

    /// Reads a command well enough to colour it, and no further.
    nonisolated static func risk(of command: String) -> Risk {
        let text = command.lowercased()
        for phrase in dangerous where text.contains(phrase) { return .danger }

        var worst = Risk.safe
        // `&&`, `||`, `;` and pipes each start a new program; the whole line is only as safe as
        // its least safe segment, so `ls && cargo install x` is not a read.
        for segment in text.split(whereSeparator: { "|&;".contains($0) }) {
            let words = segment.split(separator: " ").map(String.init)
            // Leading `FOO=bar` is not the program, the same reason the Rust side steps over it.
            guard let raw = words.first(where: { !$0.contains("=") }) else { continue }
            let program = raw.split(separator: "/").last.map(String.init) ?? raw
            if readers.contains(program) { continue }
            let pair = words.count > 1 ? "\(program) \(words[1])" : program
            if readSubcommands.contains(pair) { continue }
            worst = max(worst, .warn)
        }
        return worst
    }

    /// A run of consecutive calls to the same tool, shown as one row.
    struct Group: Identifiable {
        let id: String
        let tool: String
        var calls: [Call]
        var failed: Bool { calls.contains(where: \.failed) }
        var running: Bool { calls.contains(where: \.running) }
        var first: Call { calls[0] }
        /// The row is as risky as its worst call: a run of five `Bash`es collapsed into one line
        /// must not hide the one of them that was `rm -rf`.
        var risk: Risk { calls.map(\.risk).max() ?? .safe }
        /// What the whole run took, once all of it has finished.
        var duration: TimeInterval? {
            let done = calls.compactMap(\.duration)
            return done.count == calls.count ? done.reduce(0, +) : nil
        }
    }

    /// Consecutive calls to the same tool collapse into one row.
    ///
    /// The earlier version required the previous call to have produced no output yet, which is
    /// only true while a turn is streaming — so a replayed session collapsed nothing and rendered
    /// three hundred near-identical `Bash` rows. Grouping is about the tool, not about whether the
    /// answer has arrived, and a failure inside a run is carried by the group rather than
    /// splitting it.
    ///
    /// Stored and maintained in place, never rebuilt.
    ///
    /// It was computed and read from `body`, so every token rebuilt the whole list — and a `Call`
    /// is a struct carrying its entire output, so that was a deep copy of every byte the turn had
    /// printed, per frame. Rebuilding it on each mutation instead only moved the cost: a turn with
    /// a hundred and sixty calls does five hundred mutations, and each one copied all of them.
    /// Replaying a session did it six hundred times before a single frame was drawn, which is what
    /// "clicking a session loads all the events" looked like.
    ///
    /// So a mutation patches the one entry it touches. `place` is what makes that O(1) — results
    /// do not arrive in the order the calls were made, so finding the entry by scanning would put
    /// the scan back.
    private(set) var groups: [Group] = []
    /// Call id → where it sits in `groups`: which group, and which call within it.
    private var place: [String: (group: Int, at: Int)] = [:]

    /// A new top-level call joins the last group when it is the same tool, or starts a new one.
    private func group(_ call: Call) {
        if groups.last?.tool == call.tool {
            groups[groups.count - 1].calls.append(call)
        } else {
            groups.append(Group(id: call.id, tool: call.tool, calls: [call]))
        }
        place[call.id] = (groups.count - 1, groups[groups.count - 1].calls.count - 1)
    }

    /// Copy one call back over the group's own copy of it. The group holds values, so a change to
    /// `calls` is invisible to the rows until this runs.
    private func regroup(_ i: Int) {
        guard let at = place[calls[i].id] else { return }
        groups[at.group].calls[at.at] = calls[i]
    }
}

/// Just enough JSON to read a tool call's input.
///
/// `[String: Any]` is not `Sendable` and `AnyCodable` is a dependency for four cases.
enum JSONValue: Decodable, Sendable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case object([String: JSONValue])
    case array([JSONValue])
    case null

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode([String: JSONValue].self) { self = .object(v) }
        else if let v = try? c.decode([JSONValue].self) { self = .array(v) }
        else { self = .null }
    }

    var stringValue: String? {
        switch self {
        case .string(let s): return s.isEmpty ? nil : s
        default: return nil
        }
    }

    subscript(key: String) -> JSONValue? {
        if case .object(let o) = self { return o[key] }
        return nil
    }

    /// Back as JSON, indented. For a tool Keel has no shape for, this is what is shown — all of
    /// it, because "we kept one field of it" is the thing being fixed.
    func pretty(indent: Int) -> String {
        let pad = String(repeating: "  ", count: indent + 1)
        let close = String(repeating: "  ", count: indent)
        switch self {
        case .string(let s):
            // Enough of an escape for something that is read, not re-parsed.
            let escaped = s.replacingOccurrences(of: "\\", with: "\\\\")
                .replacingOccurrences(of: "\"", with: "\\\"")
                .replacingOccurrences(of: "\n", with: "\\n")
            return "\"\(escaped)\""
        case .number(let n): return n == n.rounded() ? String(Int(n)) : String(n)
        case .bool(let b): return b ? "true" : "false"
        case .null: return "null"
        case .array(let items):
            guard !items.isEmpty else { return "[]" }
            return "[\n" + items.map { pad + $0.pretty(indent: indent + 1) }
                .joined(separator: ",\n") + "\n\(close)]"
        case .object(let o):
            guard !o.isEmpty else { return "{}" }
            return "{\n" + o.sorted { $0.key < $1.key }
                .map { "\(pad)\"\($0.key)\": \($0.value.pretty(indent: indent + 1))" }
                .joined(separator: ",\n") + "\n\(close)}"
        }
    }

    /// A `tool_result` is a string on some records and a list of text blocks on others.
    var flatText: String {
        switch self {
        case .string(let s): return s
        case .array(let items): return items.map(\.flatText).joined(separator: " ")
        case .object(let o): return o["text"]?.flatText ?? ""
        default: return ""
        }
    }
}

extension String {
    /// Cut to a length a `Text` can afford to measure.
    ///
    /// `lineLimit` is not this. It caps the drawn height, and CoreText still encodes every
    /// character to find out where the cap falls — reported as a 2,000 ms App Hang with
    /// `TASCIIEncoder::Encode` under `LazyStack.measureEstimates`, because a lazy stack estimates
    /// every row and a prompt is often a paste. Both panes draw a turn's prompt; both need this.
    func capped(_ chars: Int) -> String {
        count <= chars ? self : String(prefix(chars)) + "…"
    }

    /// One line of the provider's stream, indented — or unchanged if it is not JSON.
    ///
    /// `JSONSerialization` rather than a printer of our own: the console shows bytes Keel did not
    /// necessarily understand, so the one thing it must not do is reformat them through a decoder
    /// that has an opinion about their shape. A line that will not parse — stderr, the exit line,
    /// the half-written record — is handed back as it came, which is the whole point of this view.
    ///
    /// Capped at both ends: a 200 KB record is not indented at all (parsing it costs more than
    /// reading it is worth), and the result is cut, because a `Text` is laid out whole.
    var indentedJSON: String {
        guard count < 100_000, let data = data(using: .utf8),
              let parsed = try? JSONSerialization.jsonObject(with: data),
              let out = try? JSONSerialization.data(
                  withJSONObject: parsed,
                  options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]),
              let text = String(data: out, encoding: .utf8)
        else { return capped(4_000) }
        return text.capped(20_000)
    }
}
