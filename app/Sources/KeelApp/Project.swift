import Foundation
import SwiftUI

/// The once-per-window root: the daemon connection and everything the project says about itself.
///
/// Every lane used to hold its own copy of the project — the scan, the session list, the trust
/// flag, thirty fields — and `adopt(project:)` copied them from a sibling whenever a lane was
/// made, closed, or the shared refresh ran. A copy is a thing that goes stale, and it did: a
/// new tab that missed the copy drew "Reading…" over data every other tab had. One object,
/// held by reference, cannot disagree with itself.
@MainActor
@Observable
final class Project {
    let client: Client
    let port: UInt16
    let store = ProjectStore()
    /// What changed, from the daemon. Started by `Lanes`, which routes it.
    let events = DaemonEvents()
    private var checkouts: [String: RepoStore] = [:]

    init(client: Client, port: UInt16) {
        self.client = client
        self.port = port
    }

    /// The git facts for one checkout — the project's tree, or a lane's worktree. One per
    /// checkout for the life of the window, so two lanes on the same tree read the same status
    /// and a lane that moves to its own branch reads its own.
    func repo(for worktree: String?) -> RepoStore {
        let key = worktree ?? ""
        if let existing = checkouts[key] { return existing }
        let made = RepoStore(worktree: worktree)
        checkouts[key] = made
        return made
    }
}

/// What the daemon says about the project as a whole. Read by every lane, written by whichever
/// refresh ran last, and never copied.
@MainActor
@Observable
final class ProjectStore {
    /// Whether the daemon has answered `/api/state` at least once. Before that, every empty
    /// list is "not loaded yet", not "nothing here".
    var loaded = false
    /// Why the last state refresh failed, when it did.
    var loadFailed: String?
    /// The open repository's path, for the project menu and for the Finder.
    var repoPath = ""
    /// Whether the daemon has a project open at all. `nil` until it has answered.
    var projectOpenKnown: Bool?
    /// Sessions in this project, newest first. Titles, counts and timestamps only — the daemon
    /// deliberately never returns message bodies to a listing.
    var sessions: [Wire.Session] = []
    var findings: [Wire.Finding] = []
    /// The whole scan: score, profile, plan. `findings` stays the list the badge counts.
    var scan: Wire.Scan?
    var workspace = Wire.Workspace(sessions: [])
    /// Whether this project is trusted, which the window says out loud for as long as it is true.
    var trusted = false
    /// The check the daemon detected, for the status bar and the empty Trace.
    var gateCommand: String?
    var slashCommands: [String] = []
    var missingSuggestions: [SkillCatalog.Entry] = []
    var tools: [ConnectionsSettings.Tool] = []
    var scopeFileLimit = 8
    var policySources: [String] = []
    var allowedProviders: Set<String> = ["claude", "codex"]
    var policyRequiresIsolation = true
    var isolationByPolicy = false
    /// The dev server command the daemon detected, and the directory it runs in.
    var devDetected: String?
    var devDir: String?
}

/// The git facts for one checkout: what git says about the tree, its history, and its files.
@MainActor
@Observable
final class RepoStore {
    /// The lane checkout this is about, or `nil` for the project's own tree.
    let worktree: String?
    /// Whether the project is under git at all. A new one is not, and that is not an error.
    var isRepo = true
    var branch: String?
    var changes: [Wire.Change] = []
    /// Every repository in the opened folder. One for an ordinary project; several when the
    /// folder is a workspace holding `backend/` and `frontend/`.
    var repos: [Wire.Repo] = []
    /// `changes` is a folded, capped view of a working tree with thousands of files in it.
    var changesCollapsed = false
    /// The last commits on this checkout, for the list beside the working tree.
    var commits: [Wire.Commit] = []
    var tree: [Wire.Node] = []
    /// Every path, flat — for the filter and the mention picker.
    var files: [String] = []
    var branches: Wire.Branches?
    /// The last read that failed, for the panel to say beside what it still has.
    var failure: Fault?
    /// Climbs on every read of the tree: the one value a diff card compares to know its file
    /// may have changed under it. It was a counter every caller had to remember to bump.
    var treeVersion = 0

    init(worktree: String?) {
        self.worktree = worktree
    }

    private func query(_ extra: [String: String] = [:]) -> [String: String] {
        var out = extra
        if let worktree { out["wt"] = worktree }
        return out
    }

    /// What git says about the tree and its last commits. The fault, when the read failed.
    func refreshGit(_ client: Client) async -> Fault? {
        switch await client.fetch("git status", "/api/git/status", query()) as Result<Wire.GitStatus, Fault> {
        case .success(let s):
            isRepo = s.isRepo
            branch = s.branch
            changes = s.changes
            repos = s.repos
            changesCollapsed = s.collapsed
            failure = nil
            treeVersion += 1
        case .failure(let fault):
            failure = fault
            return fault
        }
        commits = (try? await client.get("/api/git/log", query(["n": "20"]))) ?? []
        return nil
    }

    func refreshTree(_ client: Client) async -> Fault? {
        switch await client.fetch("the file tree", "/api/tree", query()) as Result<[Wire.Node], Fault> {
        case .success(let t):
            tree = t
            var flat: [String] = []
            func walk(_ nodes: [Wire.Node]) {
                for n in nodes {
                    if n.dir { walk(n.children ?? []) } else { flat.append(n.path) }
                }
            }
            walk(t)
            files = flat
            return nil
        case .failure(let fault):
            failure = fault
            return fault
        }
    }

    func refreshBranches(_ client: Client) async {
        branches = try? await client.get("/api/git/branches", query())
    }
}
