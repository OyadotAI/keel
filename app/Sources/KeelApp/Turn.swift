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
    var text = ""
    /// Whether any of it arrived as deltas; when none did, the whole `assistant` message is
    /// the only copy and is taken instead.
    var streamedText = false
    var thinking = ""

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
        var selector: String
        var before: NSImage?
        var after: NSImage?
        var verdict: DesignCheck.Verdict
        var duplicated: Bool
        /// What the page said moved, and a photograph of all of it together.
        var regions: [Region] = []
        var pageAfter: NSImage?
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
        var output: String = ""
        var failed = false
        var running = true
        /// Calls a subagent made on this call's behalf. Only a `Task` has any.
        var children: [Call] = []

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
    static let writeTools: Set<String> = ["Edit", "Write", "MultiEdit", "NotebookEdit", "Update"]

    func noteEdit(_ path: String?) {
        guard let path, !path.isEmpty, !files.contains(path) else { return }
        files.append(path)
    }

    /// `parent` is the `Task` call a subagent is working for. Its calls nest under that row
    /// rather than joining the top-level list: a subagent that reads forty files is one line
    /// saying so, and forty lines is the transcript nobody could follow in the terminal either.
    func begin(call id: String, tool: String, input: [String: JSONValue], parent: String? = nil) {
        // The argument worth showing, in the order the tools actually carry it.
        let subject = ["command", "file_path", "path", "pattern", "description"]
            .compactMap { input[$0]?.stringValue }
            .first ?? ""
        let oneLine = subject.split(separator: "\n").first.map(String.init) ?? ""
        let call = Call(id: id, tool: tool, subject: String(oneLine.prefix(160)))
        if let parent, let pi = callIndex[parent] {
            parentOf[id] = parent
            calls[pi].children.append(call)
        } else {
            callIndex[id] = calls.count
            calls.append(call)
        }
        // A file a subagent wrote is still a file this turn wrote.
        if Self.writeTools.contains(tool) {
            noteEdit(input["file_path"]?.stringValue ?? input["path"]?.stringValue)
        }
    }

    func finish(call id: String, output: String, failed: Bool) {
        if let parent = parentOf[id], let pi = callIndex[parent],
           let ci = calls[pi].children.firstIndex(where: { $0.id == id }) {
            calls[pi].children[ci].output = output
            calls[pi].children[ci].failed = failed
            calls[pi].children[ci].running = false
            return
        }
        guard let i = callIndex[id] else { return }
        calls[i].output = output
        calls[i].failed = failed
        calls[i].running = false
    }

    /// A run of consecutive calls to the same tool, shown as one row.
    struct Group: Identifiable {
        let id: String
        let tool: String
        var calls: [Call]
        var failed: Bool { calls.contains(where: \.failed) }
        var running: Bool { calls.contains(where: \.running) }
        var first: Call { calls[0] }
    }

    /// Consecutive calls to the same tool collapse into one row.
    ///
    /// The earlier version required the previous call to have produced no output yet, which is
    /// only true while a turn is streaming — so a replayed session collapsed nothing and rendered
    /// three hundred near-identical `Bash` rows. Grouping is about the tool, not about whether the
    /// answer has arrived, and a failure inside a run is carried by the group rather than
    /// splitting it.
    var groups: [Group] {
        var out: [Group] = []
        for c in calls {
            if var last = out.last, last.tool == c.tool {
                last.calls.append(c)
                out[out.count - 1] = last
            } else {
                out.append(Group(id: c.id, tool: c.tool, calls: [c]))
            }
        }
        return out
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
