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
    var thinking = ""

    /// Files written, in the order first touched. Order matters: it is the shape of the work.
    private(set) var files: [String] = []
    /// Tool calls in order, keyed for pairing with the result that answers them.
    private(set) var calls: [Call] = []
    private var callIndex: [String: Int] = [:]

    /// The daemon caps a replay at 300 calls and says so; hiding that would be a quiet lie about
    /// what the session did.
    var truncated = false
    var cost: Double?
    var durationMS: Int?
    var finished = false

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

    func begin(call id: String, tool: String, input: [String: JSONValue]) {
        // The argument worth showing, in the order the tools actually carry it.
        let subject = ["command", "file_path", "path", "pattern", "description"]
            .compactMap { input[$0]?.stringValue }
            .first ?? ""
        let oneLine = subject.split(separator: "\n").first.map(String.init) ?? ""
        callIndex[id] = calls.count
        calls.append(Call(id: id, tool: tool, subject: String(oneLine.prefix(160))))
        if Self.writeTools.contains(tool) {
            noteEdit(input["file_path"]?.stringValue ?? input["path"]?.stringValue)
        }
    }

    func finish(call id: String, output: String, failed: Bool) {
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
enum JSONValue: Decodable {
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
