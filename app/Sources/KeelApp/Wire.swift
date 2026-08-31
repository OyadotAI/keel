import Foundation

/// The shapes `keel serve` actually sends.
///
/// Hand-written rather than generated, and deliberately partial: a native client that fails to
/// decode a whole response because the daemon grew a field is worse than one that ignores it.
/// Every optional here is optional because the server can genuinely omit it.
enum Wire {
    struct State: Decodable, Sendable {
        var repo: String
        var projectOpen: Bool
        var scan: Scan
        var workspace: Workspace
        var policy: Policy?

        enum CodingKeys: String, CodingKey {
            case repo
            case projectOpen = "project_open"
            case scan, workspace, policy
        }
    }

    struct Policy: Decodable, Sendable {
        var maxFiles: Int
        var requireIsolation: Bool
        /// A policy *file* demanded isolation, rather than it being Keel's own default. Only then
        /// is a failed checkout a reason to refuse the turn rather than to run in the project.
        var isolationByPolicy: Bool?
        var allowedProviders: [String]
        var sources: [String]

        enum CodingKeys: String, CodingKey {
            case maxFiles = "max_files"
            case requireIsolation = "require_isolation"
            case isolationByPolicy = "isolation_by_policy"
            case allowedProviders = "allowed_providers"
            case sources
        }
    }

    struct Scan: Decodable, Sendable {
        var score: Int
        var findings: [Finding]
        var profile: Profile?
        @DefaultEmpty var plan: [Phase]
        /// Findings the team set aside; they are not in `findings`.
        @DefaultEmpty var ignored: [String]
    }

    /// What the scanner thinks the repository is and where it runs.
    struct Profile: Decodable, Sendable {
        var template: String
        var template_title: String
        var like: String
        var confidence: Int
        @DefaultEmpty var signals: [String]
        @DefaultEmpty var hosting: [String]
        @DefaultEmpty var stack: [String]
    }

    /// One step of the road to production: a title, why, and the finding ids in it.
    struct Phase: Decodable, Identifiable, Sendable {
        var title: String
        var why: String
        @DefaultEmpty var findings: [String]
        var id: String { title }
    }

    struct Finding: Decodable, Identifiable, Sendable {
        var id: String
        var severity: String
        var title: String
        var detail: String
        var path: String?
    }

    struct Workspace: Decodable, Sendable {
        var sessions: [Session]
        @DefaultEmpty var skills: [Named]
        @DefaultEmpty var agents: [Named]
        @DefaultEmpty var plugins: [Plugin]
        @DefaultEmpty var hooks: [Hook]
        @DefaultEmpty var mcpServers: [Named]

        enum CodingKeys: String, CodingKey {
            case sessions, skills, agents, plugins, hooks
            case mcpServers = "mcp_servers"
        }

        init(sessions: [Session]) {
            self.sessions = sessions
            _skills = DefaultEmpty()
            _agents = DefaultEmpty()
            _plugins = DefaultEmpty()
            _hooks = DefaultEmpty()
            _mcpServers = DefaultEmpty()
        }
    }

    /// Skills, subagents and MCP servers all answer the same three questions, so they share a
    /// shape rather than three near-identical structs.
    struct Named: Decodable, Identifiable, Sendable, Equatable {
        var name: String
        @DefaultEmptyString var description: String
        var scope: String
        /// Where it lives on disk. Absent for MCP servers, which are config entries not files.
        var path: String?
        var id: String { scope + name }
        /// Anything that arrived *with the repository* was written by whoever wrote the repo.
        var fromRepo: Bool { scope == "project" }

        // Compared by identity: the property wrappers block a synthesised conformance, and what
        // the inspector asks is "is this the same row", not "is every field equal".
        static func == (a: Named, b: Named) -> Bool { a.id == b.id }
    }

    struct Plugin: Decodable, Identifiable, Sendable, Equatable {
        var name: String
        var enabled: Bool
        @DefaultEmptyString var marketplace: String
        var scope: String
        var id: String { scope + name }
        static func == (a: Plugin, b: Plugin) -> Bool { a.id == b.id }
    }

    /// A hook is a shell command that runs on the machine of whoever opens the repository, which
    /// is why the scope is the first thing shown about one.
    struct Hook: Decodable, Identifiable, Sendable, Equatable {
        var event: String
        var command: String
        var scope: String
        @DefaultEmptyString var source: String
        var id: String { event + source + command }
        var fromRepo: Bool { scope == "project" }
        static func == (a: Hook, b: Hook) -> Bool { a.id == b.id }
    }

    struct Session: Decodable, Identifiable, Hashable, Sendable {
        var id: String
        var title: String?
        var messages: Int
        var lastActive: String?
        /// `here`, `above` or `below` the repository.
        var scope: String?
        var cwd: String?

        enum CodingKeys: String, CodingKey {
            case id, title, messages, scope, cwd
            case lastActive = "last_active"
        }

        /// The folder it was started in, when that is not the repository.
        var elsewhere: String? {
            guard scope != nil, scope != "here", let cwd else { return nil }
            return (cwd as NSString).lastPathComponent
        }
    }

    /// One entry in the repository tree. Recursive, and the whole tree arrives at once.
    struct Node: Decodable, Identifiable {
        var name: String
        var path: String
        var dir: Bool
        var children: [Node]?
        var id: String { path }
    }

    /// One exchange from a transcript, as the daemon summarises it.
    struct Turn: Decodable, Sendable {
        var role: String
        var text: String
    }

    struct GitStatus: Decodable {
        var isRepo: Bool
        var branch: String?
        var changes: [Change]
        /// One entry for an ordinary project; two or more when the folder is a workspace.
        var repos: [Repo] = []

        enum CodingKeys: String, CodingKey {
            case isRepo = "is_repo"
            case branch, changes, repos
        }
    }

    struct Change: Decodable, Identifiable {
        var path: String
        var status: String
        var label: String
        var id: String { path }
    }

    /// One git repository inside the opened folder.
    ///
    /// A folder people open is often a workspace rather than a project — `backend/` and
    /// `frontend/`, each its own repository — and Keel used to report that as "not a git
    /// repository" and offer to `git init` a third one around them.
    struct Repo: Decodable, Identifiable, Sendable {
        /// Relative to the opened folder; empty when the folder is itself the repository.
        var dir: String
        var branch: String?
        var changes: [Change]
        var id: String { dir }
        /// What to call it on screen.
        var name: String { dir.isEmpty ? "project" : dir }
    }

    struct Branch: Decodable, Identifiable, Sendable {
        var name: String
        var current: Bool
        var upstream: String?
        var ahead: Int
        var behind: Int
        var subject: String
        var id: String { name }
    }

    struct Branches: Decodable, Sendable {
        var current: String?
        var local: [Branch]
        var remote: [String]
        var remotes: [String]
        var staged: Int
        var unstaged: Int
    }

    /// One commit on the current branch.
    struct Commit: Decodable, Identifiable, Sendable {
        var sha: String
        var subject: String
        var when: Int
        var files: Int
        var pushed: Bool
        var id: String { sha }
        var date: Date { Date(timeIntervalSince1970: TimeInterval(when)) }
    }

    struct Diff: Decodable {
        var path: String
        var hunks: [Hunk]
        var untracked: Bool
        /// Where these hunks came from when they are not the working tree — the commit that
        /// already holds them — or why there are none. Nil for an ordinary uncommitted change.
        var note: String?
    }

    struct Hunk: Decodable {
        var header: String
        var lines: [DiffLine]
    }

    struct DiffLine: Decodable {
        /// `add`, `del`, or `ctx`.
        var kind: String
        var old: Int?
        var new: Int?
        var text: String
    }

    /// A background command Keel is running on the conversation's behalf.
    ///
    /// It belongs to the daemon rather than to the turn because a turn is one `claude -p` and the
    /// CLI kills its own background shells at teardown — which is why "I'll watch it and report"
    /// used to be a sentence nothing behind it could keep.
    struct Job: Decodable, Identifiable, Equatable {
        var id: String
        var lane: String
        var command: String
        var dir: String
        var started: Double
        var finished: Double?
        var exit: Int?
        var log: [String]
        var reported: Bool

        var running: Bool { finished == nil }

        /// How long it has been going, or how long it took.
        var elapsed: TimeInterval { (finished ?? Date().timeIntervalSince1970) - started }
    }

    /// A tool call the agent is blocked on.
    struct Pending: Decodable, Identifiable {
        var id: String
        var tool: String
        var command: String
        var rules: [String]
        var sessionId: String
        /// The whole tool input. Only a question reads it.
        var input: JSONValue?

        enum CodingKeys: String, CodingKey {
            case id, tool, command, rules, input
            case sessionId = "session_id"
        }

        var isQuestion: Bool { tool == "AskUserQuestion" }

        /// The agent wants to leave a command running behind it. Not a permission — the answer
        /// decides who runs it, not whether it is allowed — so it gets a card of its own.
        var isMonitor: Bool { tool == "MonitorRequest" }

        /// The questions an `AskUserQuestion` carries, or none for a permission.
        var questions: [Question] {
            guard case .array(let qs)? = input?["questions"] else { return [] }
            return qs.compactMap { q in
                guard let text = q["question"]?.stringValue else { return nil }
                let options: [Option]
                if case .array(let os)? = q["options"] {
                    options = os.compactMap { o in
                        o["label"]?.stringValue.map {
                            Option(label: $0, detail: o["description"]?.stringValue ?? "")
                        }
                    }
                } else { options = [] }
                let multi: Bool
                if case .bool(let b)? = q["multiSelect"] { multi = b } else { multi = false }
                return Question(text: text, options: options, multiSelect: multi)
            }
        }
    }

    struct Question: Identifiable {
        var text: String
        var options: [Option]
        var multiSelect: Bool
        var id: String { text }
    }

    /// One answer, and what choosing it means. The answer sent back is the label alone.
    struct Option: Identifiable, Hashable {
        var label: String
        var detail: String
        var id: String { label }
    }

    /// A lane's own checkout.
    struct Worktree: Decodable, Identifiable, Sendable {
        var name: String
        var branch: String
        var path: String
        var ahead: Int
        var dirty: Bool
        var id: String { name }
    }

    struct Check: Decodable {
        var command: String
        var source: String
    }

    struct Problem: Decodable, Identifiable, Equatable {
        var file: String
        var line: Int
        var col: Int
        var severity: String
        var message: String
        var id: String { "\(file):\(line):\(col):\(message)" }
    }
}


/// A missing list is an empty list, and a missing string is an empty one.
///
/// The daemon can grow or drop a field, and a native client that refuses to decode a whole response
/// over one absent key is worse than one that shows a shorter list.
@propertyWrapper
struct DefaultEmpty<T: Decodable & Sendable>: Decodable, Sendable {
    var wrappedValue: [T]
    init(from decoder: Decoder) throws {
        wrappedValue = (try? [T](from: decoder)) ?? []
    }
    init() { wrappedValue = [] }
}

@propertyWrapper
struct DefaultEmptyString: Decodable, Sendable {
    var wrappedValue: String
    init(from decoder: Decoder) throws {
        wrappedValue = (try? String(from: decoder)) ?? ""
    }
    init() { wrappedValue = "" }
}

extension KeyedDecodingContainer {
    func decode<T>(_ type: DefaultEmpty<T>.Type, forKey key: Key) throws -> DefaultEmpty<T> {
        (try? decodeIfPresent(type, forKey: key)) ?? DefaultEmpty<T>()
    }
    func decode(_ type: DefaultEmptyString.Type, forKey key: Key) throws -> DefaultEmptyString {
        (try? decodeIfPresent(type, forKey: key)) ?? DefaultEmptyString()
    }
}
