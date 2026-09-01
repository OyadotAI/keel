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

    init(worktree: String?) {
        self.worktree = worktree
    }
}
