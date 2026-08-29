import SwiftUI

/// Every session in this project, running at the same time, in one window.
///
/// This is the whole argument for the application existing. One agent in one pane is a terminal
/// with nicer chrome; the reason to build a window is that three of them can be working at once
/// and you can see all three without switching anything. The daemon already supports it — each
/// `/api/chat` spawns its own `claude`, and approvals and one-time permission rules are keyed by
/// session so a question reaches the lane that asked it.
///
/// Sessions were separate windows before this, which is the same mistake in a different shape:
/// you still could not see them together.
@MainActor
@Observable
final class Lanes {
    let client: Client
    let port: UInt16

    private(set) var lanes: [SessionModel] = []
    var activeID: UUID?

    init(client: Client, port: UInt16) {
        self.client = client
        self.port = port
        lanes = []
        let first = SessionModel(client: client, port: port)
        first.lanes = self
        lanes = [first]
        activeID = first.id
    }

    // MARK: - Remembering what was open
    //
    // Which conversations you had open is per-machine UI state, not something the repository or
    // the CLI needs to agree with, so it lives in `UserDefaults` beside the recents list. Keyed by
    // project, because "the sessions I had open" means nothing without one.

    private static func key(_ repo: String) -> String { "keel.lanes." + repo }

    private struct Saved: Codable {
        var sessions: [String]
        var active: Int
        /// Parallel to `sessions`: the lane's checkout, or nothing for one on the project's tree.
        /// Optional at the top level too, so the file written before lanes had checkouts still
        /// decodes.
        var worktrees: [String?]?
    }

    /// Save the Claude Code session ids of every lane that has one.
    ///
    /// A lane with no session id has never run a turn, so there is nothing to come back to — and
    /// restoring a row of empty lanes would be restoring the appearance of work rather than work.
    func remember(repo: String) {
        guard !repo.isEmpty else { return }
        let with = lanes.filter { $0.sessionId != nil }
        let ids = with.compactMap(\.sessionId)
        let activeIndex = lanes.firstIndex { $0.id == activeID }
            .map { i in lanes[..<i].count { $0.sessionId != nil } } ?? 0
        let saved = Saved(sessions: ids, active: min(activeIndex, max(ids.count - 1, 0)),
                          worktrees: with.map(\.worktree))
        if let data = try? JSONEncoder().encode(saved) {
            UserDefaults.standard.set(data, forKey: Self.key(repo))
        }
    }

    /// Reopen what was open, replaying each conversation.
    ///
    /// Every launch used to give you one empty lane, so the session you were in the middle of was
    /// something you had to go and find in History — every time.
    func restore(repo: String) async {
        guard !repo.isEmpty,
              let data = UserDefaults.standard.data(forKey: Self.key(repo)),
              let saved = try? JSONDecoder().decode(Saved.self, from: data),
              !saved.sessions.isEmpty else { return }

        await refreshWorktrees()
        let checkouts = Set(worktrees.map(\.name))

        // Only sessions the daemon can still see: a transcript can be deleted, and a lane pointing
        // at one that is gone would fail on its first turn rather than on open. A lane in its own
        // checkout keeps its sessions there, so for those the checkout still existing is the test.
        let known = Set(lanes.first?.sessions.map(\.id) ?? [])
        let pairs = zip(saved.sessions, saved.worktrees ?? Array(repeating: nil, count: saved.sessions.count))
        let live = pairs.filter { id, wt in
            if let wt { return checkouts.contains(wt) }
            return known.isEmpty || known.contains(id)
        }
        guard !live.isEmpty else { return }

        var restored: [SessionModel] = []
        for (id, wt) in live {
            let m = SessionModel(client: client, port: port, sessionId: id)
            m.lanes = self
            m.worktree = wt
            m.isolated = wt != nil
            if let a = lanes.first { m.adopt(project: a) }
            restored.append(m)
            await m.open(session: id)
            if wt != nil { await m.refreshGit(); await m.refreshTree() }
        }
        lanes = restored
        activeID = restored[min(saved.active, restored.count - 1)].id
    }

    /// Never nil: `close` refills an emptied list, and the window reads this on every pass.
    var active: SessionModel {
        if let found = lanes.first(where: { $0.id == activeID }) ?? lanes.first { return found }
        let m = SessionModel(client: client, port: port)
        m.lanes = self
        lanes = [m]
        activeID = m.id
        return m
    }

    /// How many lanes are doing something right now — the number the window title and the Dock
    /// badge care about.
    var runningCount: Int { lanes.count { $0.running } }
    var waitingCount: Int { lanes.reduce(0) { $0 + $1.pending.count } }

    /// Whether any other lane is mid-turn — the condition under which a second agent editing the
    /// same working tree can clobber the first.
    var wouldOverlap: Bool { lanes.contains { $0.running } }

    /// Every lane checkout the project has, with how far each has gone.
    private(set) var worktrees: [Wire.Worktree] = []

    func refreshWorktrees() async {
        worktrees = (try? await client.get("/api/worktree")) ?? []
    }

    func worktree(of lane: SessionModel) -> Wire.Worktree? {
        lane.worktree.flatMap { name in worktrees.first { $0.name == name } }
    }

    struct FinishBody: Encodable { var name: String; var message: String }
    struct DiscardBody: Encodable { var name: String; var force: Bool }

    /// Merge a lane's work into the project and close it. The daemon refuses rather than
    /// guesses — a dirty project, a conflict — and the refusal is shown on the lane.
    func finish(_ lane: SessionModel, message: String) async {
        guard let name = lane.worktree else { return }
        do {
            _ = try await client.post("/api/worktree/finish",
                                      body: FinishBody(name: name, message: message), as: Bool.self)
            close(lane)
            await refreshShared()
        } catch {
            lane.lastError = error.localizedDescription
        }
    }

    /// Throw a lane away. Without `force` the daemon refuses when commits would be lost, and
    /// says how many; the row turns that into the confirmation.
    func discard(_ lane: SessionModel, force: Bool) async -> String? {
        guard let name = lane.worktree else { close(lane); return nil }
        do {
            _ = try await client.post("/api/worktree/discard",
                                      body: DiscardBody(name: name, force: force), as: Bool.self)
            close(lane)
            await refreshShared()
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    @discardableResult
    func newLane(resuming id: String? = nil, isolated: Bool = false) -> SessionModel {
        // An empty lane you never typed into is not a second agent, it is a second row. Focusing
        // the one that already exists is what the click meant.
        //
        // "Empty" has to mean no session as well as no turns. Checking turns alone meant a lane
        // holding a real conversation whose transcript had not loaded yet — a restored one, on
        // launch — counted as spare, and the next `+` quietly took it over.
        if id == nil,
           let idle = lanes.first(where: { $0.turns.isEmpty && $0.sessionId == nil && !$0.running }) {
            activeID = idle.id
            idle.isolated = isolated
            return idle
        }
        let m = SessionModel(client: client, port: port, sessionId: id)
        m.lanes = self
        m.isolated = isolated
        Telemetry.track("lane_created", ["isolated": isolated, "resumed": id != nil])
        // From whichever lane exists, not from `active` — which would refill an emptied list
        // with a lane of its own on the way to making this one.
        if let a = lanes.first(where: { $0.id == activeID }) ?? lanes.first { m.adopt(project: a) }
        lanes.append(m)
        activeID = m.id
        return m
    }

    /// Open a past session in a lane. If it is already open, focus it rather than opening a second
    /// copy that would then race the first over `--resume`.
    func open(session id: String) async {
        if let existing = lanes.first(where: { $0.sessionId == id }) {
            activeID = existing.id
            // Already open: bring it back to the end rather than leaving it where it was parked.
            existing.pinTick += 1
            return
        }
        let m = newLane(resuming: id)
        await m.open(session: id)
    }

    func close(_ model: SessionModel) {
        model.stop()
        lanes.removeAll { $0.id == model.id }
        // A window with no lane has nothing to show, so closing the last one starts a fresh one
        // rather than leaving an empty frame.
        if lanes.isEmpty { newLane() }
        if activeID == model.id { activeID = lanes.last?.id }
    }

    /// Reload the shared, project-level state once, rather than once per lane.
    ///
    /// Waits for the daemon. The window's first refresh races the daemon's own startup — they are
    /// separate tasks on the same view — so the first `/api/state` used to fail, leave the project
    /// path empty, and take the session restore down with it, because that is keyed by project.
    /// Nothing retried, so the app sat on an empty lane with a nameless project until you opened
    /// something by hand.
    func refreshShared() async {
        let a = active

        for attempt in 0..<40 where a.repoPath.isEmpty {
            await a.refreshState()
            if !a.repoPath.isEmpty || a.projectOpenKnown == false { break }
            try? await Task.sleep(for: .milliseconds(attempt < 10 ? 50 : 150))
        }
        await a.refreshState()
        await a.refreshGit()
        await a.refreshTree()
        await a.refreshTrust()
        await a.refreshGatePlan()
        await a.refreshDev()
        await a.refreshSuggestions()
        await a.refreshTools()
        await refreshWorktrees()
        a.offerSetup()
        // Every lane draws the same project chrome, so they share what the project says about
        // itself rather than each asking.
        for lane in lanes where lane.id != a.id {
            lane.adopt(project: a)
        }
    }
}
