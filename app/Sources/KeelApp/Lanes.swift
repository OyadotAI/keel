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
    /// The one project every lane reads. See `Project`.
    let project: Project

    private(set) var lanes: [SessionModel] = []
    var activeID: UUID?

    /// The window is showing the start screen rather than a project.
    ///
    /// Closing the last tab used to open a blank one in its place, which is a window that looks
    /// busy and is not — and there was no way back to the screen you pick a project from without
    /// quitting. It is view state and not the daemon's: the daemon still holds the project open,
    /// so a state refresh would flip `projectOpen` back the moment it landed.
    var atStart = false

    /// Bumped whenever the project changes, so work started for the old one stops.
    ///
    /// `restore` is a sequential loop of per-lane network calls — `open`, `refreshGit`,
    /// `refreshTree` — and the project switch that should end it arrives as a *detached* `Task`
    /// from `onChange(of: repoPath)`, which cancels nothing. So both ran, and every lane the old
    /// loop had left asked about the old project's checkout against the new project's root: a 400
    /// each, reported to Sentry as a daemon failure, for a window that had simply moved on.
    private var generation = 0

    init(client: Client, port: UInt16) {
        self.client = client
        self.port = port
        project = Project(client: client, port: port)
        lanes = []
        let first = SessionModel(client: client, port: port, project: project)
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
        var tasks: [UUID]?
        var titles: [String]?
        var providers: [SessionModel.Provider]?
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
                          worktrees: with.map(\.worktree), tasks: with.map(\.id),
                          titles: with.map(\.title), providers: with.map(\.provider))
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

        // Everything below is for *this* project. Checked after each await, because the switch
        // that invalidates it does not cancel this task.
        let mine = generation
        await refreshWorktrees()
        guard mine == generation else { return }
        let checkouts = Set(worktrees.map(\.name))

        // Only sessions the daemon can still see: a transcript can be deleted, and a lane pointing
        // at one that is gone would fail on its first turn rather than on open. A lane in its own
        // checkout keeps its sessions there, so for those the checkout still existing is the test.
        let known = Set(project.store.sessions.map(\.id))
        let pairs = zip(saved.sessions, saved.worktrees ?? Array(repeating: nil, count: saved.sessions.count))
        let live = pairs.enumerated().filter { _, pair in
            let (id, wt) = pair
            if let wt { return checkouts.contains(wt) }
            return known.isEmpty || known.contains(id)
        }
        guard !live.isEmpty else { return }

        var restored: [SessionModel] = []
        for (index, pair) in live {
            let (id, wt) = pair
            let taskID = saved.tasks.flatMap { $0.indices.contains(index) ? $0[index] : nil } ?? UUID()
            let m = SessionModel(client: client, port: port, sessionId: id, id: taskID, project: project)
            m.lanes = self
            m.worktree = wt
            m.isolated = wt != nil
            m.title = saved.titles.flatMap { $0.indices.contains(index) ? $0[index] : nil } ?? m.title
            m.provider = saved.providers.flatMap { $0.indices.contains(index) ? $0[index] : nil } ?? .claude
            restored.append(m)
            await m.open(session: id)
            guard mine == generation else { return }
            if wt != nil { await m.refreshGit(); await m.refreshTree() }
            guard mine == generation else { return }
        }
        // Never over the top of a project that arrived while this was running: `restored` holds
        // the previous project's conversations and its checkouts.
        guard mine == generation, !restored.isEmpty else { return }
        lanes = restored
        activeID = restored[min(saved.active, restored.count - 1)].id
    }

    /// Everything on screen belongs to the project that is open, so a project switch clears it.
    ///
    /// `restore(repo:)` ran once, in the window's `.task`, and nothing re-ran it — so opening a
    /// different project left the old one's tabs in the strip, pointing at its sessions and its
    /// worktrees. Those lanes could not even be closed: a lane with a checkout has no close button
    /// because the choice is meant to be merge or discard, and both of those were about a
    /// repository that was no longer open.
    ///
    /// It also corrupted what was saved. `remember` writes under whatever project is open *now*,
    /// and it fires whenever the lane list changes — so the old project's lanes were being written
    /// into the new project's saved list, and restoring it later would have reopened someone
    /// else's conversations.
    ///
    /// The lane that did the opening is kept, because `openProject` has already reset it; its
    /// checkout is cleared because that belonged to the old repository.
    func switchProject(to repo: String) async {
        generation += 1
        let keep = active
        // `closed()`, not `stop()`: these lanes are being dropped from the window, so their
        // background-job loops go with them. `stop()` deliberately leaves that loop running,
        // which is right when a turn ends and wrong when the lane does — otherwise a discarded
        // lane keeps polling for jobs, in a project that is no longer open.
        for lane in lanes where lane.id != keep.id { lane.closed() }
        keep.worktree = nil
        keep.isolated = false
        lanes = [keep]
        activeID = keep.id
        await refreshWorktrees()
        await restore(repo: repo)
    }

    /// Never nil: `close` refills an emptied list, and the window reads this on every pass.
    var active: SessionModel {
        if let found = lanes.first(where: { $0.id == activeID }) ?? lanes.first { return found }
        let m = SessionModel(client: client, port: port, project: project)
        m.lanes = self
        lanes = [m]
        activeID = m.id
        return m
    }

    /// The lanes the rail actually draws. A hidden lane — the background Staff review — is real
    /// work but not a tab, so it must not be counted in a number the person is meant to reconcile
    /// with what is on screen, and ⌘3 must not select something with no tab to highlight.
    var shown: [SessionModel] { lanes.filter { !$0.hidden } }

    /// How many lanes are doing something right now — the number the window title and the Dock
    /// badge care about. "2 running" beside one visible spinner is a number nobody can check.
    var runningCount: Int { shown.count { $0.running } }
    var waitingCount: Int { shown.reduce(0) { $0 + $1.pending.count } }

    /// Whether *another* lane is mid-turn — the condition under which a second agent editing the
    /// same working tree can clobber the first.
    ///
    /// It used to include the asking lane, so a lane warned about itself: the composer of the only
    /// running lane told its own author that another feature was editing right now. It disagreed
    /// with `writingElsewhere`, which excludes self, about what "another lane" means.
    func wouldOverlap(with lane: SessionModel) -> Bool {
        shown.contains { $0.id != lane.id && $0.running }
    }

    /// Every lane checkout the project has, with how far each has gone.
    private(set) var worktrees: [Wire.Worktree] = []

    func refreshWorktrees() async {
        guard let live: [Wire.Worktree] = try? await client.get("/api/worktree") else { return }
        worktrees = live

        // A lane pointing at a checkout that is not there sends `?wt=` on every request and is
        // refused every time — "the feature X has no checkout in this project", forever, with no
        // way to act on it. It happens when a worktree is removed outside Keel, and it happened on
        // every project switch until switching cleared the lanes. Falling back to the project is
        // not a silent loss: the branch is still on disk, and the lane is usable again.
        let names = Set(live.map(\.name))
        for lane in lanes where lane.worktree.map({ !names.contains($0) }) == true {
            lane.worktree = nil
            lane.isolated = false
        }
    }

    func worktree(of lane: SessionModel) -> Wire.Worktree? {
        lane.worktree.flatMap { name in worktrees.first { $0.name == name } }
    }

    /// Checkouts with no tab looking at them.
    ///
    /// Closing a lane is honestly labelled "the branch and its checkout stay", and then nothing in
    /// the app ever mentioned what stayed. Measured on Keel's own repository: eight of them,
    /// 43 GB, three with no commits and no diff at all — every one built from a lane somebody
    /// closed. The list was already here; it was only ever used to look up an *open* lane's
    /// checkout.
    var orphans: [Wire.Worktree] {
        let claimed = Set(lanes.compactMap(\.worktree))
        return worktrees.filter { !claimed.contains($0.name) }
    }

    /// Put a checkout that has no tab back in front of somebody, in a lane of its own.
    func reopen(_ checkout: Wire.Worktree) {
        let lane = newLane(isolated: true)
        lane.worktree = checkout.name
        lane.title = checkout.name
        Task {
            await lane.refreshGit()
            await lane.refreshTree()
        }
    }

    struct FinishBody: Encodable { var name: String; var message: String }
    struct DiscardBody: Encodable { var name: String; var force: Bool }

    /// Merge a lane's work into the project and close it. The daemon refuses rather than
    /// guesses — a dirty project, a conflict — and the refusal is shown on the lane.
    /// What finishing actually does, said the same way wherever it is offered.
    static func finishBlurb(_ checkout: Wire.Worktree?) -> String {
        let branch = checkout?.branch ?? "its branch"
        // Where it lands, named. The sentence used to say "into the project", which is not a
        // branch, and the menu item above it read the *focused* lane's branch rather than this
        // one's — so with several features open the one sentence naming where your work goes was
        // reliably wrong.
        let into = checkout?.base.map { "“\($0)”" } ?? "the project's branch"
        return "Commits everything in the feature and merges \(branch) into \(into). "
            + "The feature's checkout is removed; the branch is deleted only once it is merged."
    }

    /// Finish, with the project's checks run first.
    ///
    /// This is what makes the Review pane the gate rather than a page that reports one. The checks
    /// run at the moment of the decision, against the tree being merged, so there is no window in
    /// which a recorded pass is about something else — and no staleness rule to get wrong. When
    /// they already ran and nothing has changed since, they are not run twice.
    ///
    /// A failure leaves the lane exactly as it was. Nothing is committed, nothing is merged, and
    /// the problems are on the screen the person is already looking at.
    func finishChecked(_ lane: SessionModel, message: String) async {
        if !lane.gateIsCurrent {
            await lane.runGateNow()
            if case .failed = lane.latestGate {
                lane.lastError = "The project's checks failed, so nothing was merged. "
                    + "The problems are in Review."
                return
            }
        }
        await finish(lane, message: message)
    }

    func finish(_ lane: SessionModel, message: String) async {
        guard let name = lane.worktree else { return }
        if let blocker = lane.mergeBlocker {
            lane.lastError = blocker
            return
        }
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
        if let why = await discardCheckout(name: name, force: force) { return why }
        close(lane)
        return nil
    }

    /// Throw away a checkout nobody has a tab on. Same refusal, same daemon, no lane to close.
    func discardCheckout(_ checkout: Wire.Worktree, force: Bool) async -> String? {
        await discardCheckout(name: checkout.name, force: force)
    }

    private func discardCheckout(name: String, force: Bool) async -> String? {
        do {
            _ = try await client.post("/api/worktree/discard",
                                      body: DiscardBody(name: name, force: force), as: Bool.self)
            await refreshShared()
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    @discardableResult
    func newLane(resuming id: String? = nil, isolated: Bool = false) -> SessionModel {
        // Anything that makes a lane is leaving the start screen. `close` sets the flag *after*
        // its own call to this, which is the one case where that order matters.
        atStart = false
        // An empty lane you never typed into is not a second agent, it is a second row. Focusing
        // the one that already exists is what the click meant.
        //
        // "Empty" has to mean no session as well as no turns. Checking turns alone meant a lane
        // holding a real conversation whose transcript had not loaded yet — a restored one, on
        // launch — counted as spare, and the next `+` quietly took it over.
        //
        // A hidden lane is never the spare. It is the background Staff review — real work with no
        // tab — and it is idle by this test for the whole time it is starting up, so `+` could
        // hand the person a tab onto somebody else's job. It is also why closing every tab while
        // one ran left `shown` empty after the replacement: the replacement *was* the hidden one.
        if id == nil,
           let idle = lanes.first(where: {
               $0.turns.isEmpty && $0.sessionId == nil && !$0.running && !$0.hidden
           }) {
            activeID = idle.id
            idle.isolated = isolated
            // Everything the last occupant chose, not just isolation. A lane that had been
            // switched to Codex and then emptied came back as a Codex lane under a menu item
            // that never mentioned an agent, and kept the old lane's title until the first send.
            idle.provider = .claude
            idle.mode = "acceptEdits"
            idle.baseBranch = nil
            idle.nextSystem = nil
            idle.chosenName = ""
            idle.title = "Untitled"
            return idle
        }
        let m = SessionModel(client: client, port: port, sessionId: id, project: project)
        m.lanes = self
        m.isolated = isolated
        // Reading from the moment the tab exists. `open(session:)` sets this, but only once the
        // caller gets round to awaiting it — so a resumed lane drew the "what a second agent is
        // for" hint first, replaced it with the spinner, and then replaced that with the
        // conversation. Three states for one click.
        m.replaying = id != nil
        Telemetry.track("lane_created", ["isolated": isolated, "resumed": id != nil])
        lanes.append(m)
        activeID = m.id
        return m
    }

    /// A lane for a review, beside the one being worked in rather than instead of it. Made in
    /// the background: the person's lane stays focused and the review tab fills in on its own.
    func reviewLane(beside current: SessionModel) -> SessionModel {
        let keep = activeID
        let m = SessionModel(client: client, port: port, sessionId: nil, project: project)
        m.lanes = self
        m.title = "Staff review"
        m.hidden = true
        lanes.append(m)
        activeID = keep
        Telemetry.track("lane_created", ["isolated": false, "resumed": false, "review": true])
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
        // A session belongs to the checkout it ran in, and only `restore` knew that — it saves the
        // name alongside the session id. Reopening the same conversation from History built a lane
        // with no `worktree` at all, so every checkout-scoped request went to the project root
        // instead: the file tree and the Changes panel showed the root's, and `/api/attach` wrote
        // a pasted block into the root's `.keel/attachments`, leaving the `@path` in the prompt
        // pointing at nothing the agent could read from its own checkout.
        await refreshWorktrees()
        if let cwd = (project.store.sessions.first { $0.id == id })?.cwd,
           cwd.contains("/.keel/worktrees/"),
           let wt = worktrees.first(where: { (cwd as NSString).lastPathComponent == $0.name }) {
            m.worktree = wt.name
            m.isolated = true
        }
        await m.open(session: id)
    }

    /// A lane other than this one that is running a turn which can write the shared working tree.
    ///
    /// Only unisolated, non-plan lanes count: a lane with a worktree of its own writes somewhere
    /// else, and a plan turn writes nothing at all.
    /// `running` alone was not the question. It goes false the moment the stream ends, and the
    /// two things that actually touch the tree happen after that: the gate, which is minutes, and
    /// then the auto-commit's `git add -A`. A lane that started in that window had its
    /// half-written files swept into somebody else's commit under somebody else's prompt — the
    /// exact corruption the caller's comment says it prevents. `settling` covers the tail.
    func writingElsewhere(than lane: SessionModel) -> String? {
        lanes.first {
            $0.id != lane.id && ($0.running || $0.settling) && !$0.isolated && $0.mode != "plan"
        }?.title
    }

    func close(_ model: SessionModel) {
        model.closed()
        lanes.removeAll { $0.id == model.id }
        // A window with no lane has nothing to draw, and `active` would conjure one anyway — so
        // the replacement is still made, and it still inherits the project from the lane just
        // closed (`newLane` copies that from a sibling, and the last tab leaves no sibling). What
        // changed is what the window shows over it: closing everything returns you to the screen
        // you pick a project from, rather than to a blank tab that looks like work.
        // `shown`, not `lanes`: a hidden lane is real work with no tab, so closing every tab
        // while the background review runs leaves a window with no tabs, a workbench drawn for a
        // lane nothing can select, and no way back to the start screen.
        if shown.isEmpty {
            newLane()
            atStart = true
        }
        if activeID == model.id { activeID = shown.last?.id }
    }

    /// Close several at once, the way a browser does.
    ///
    /// One call rather than a loop at each menu item, because `close` has a tail — the
    /// replacement lane, `atStart`, the active id — and running it per lane would make and
    /// discard a replacement for every tab on the way down.
    func close(_ doomed: [SessionModel]) {
        guard !doomed.isEmpty else { return }
        let ids = Set(doomed.map(\.id))
        for lane in doomed { lane.closed() }
        lanes.removeAll { ids.contains($0.id) }
        if shown.isEmpty {
            newLane()
            atStart = true
        }
        if let active = activeID, ids.contains(active) { activeID = shown.last?.id }
    }

    /// The other tabs, the ones before this one, and the ones after — as the menu needs them.
    ///
    /// Ordered by what the rail draws, not by `lanes`: a hidden lane is real work with no tab, so
    /// "close the tabs to the right" must not silently take one with it.
    func others(than lane: SessionModel) -> [SessionModel] {
        shown.filter { $0.id != lane.id }
    }

    func before(_ lane: SessionModel) -> [SessionModel] {
        guard let index = shown.firstIndex(where: { $0.id == lane.id }) else { return [] }
        return Array(shown[..<index])
    }

    func after(_ lane: SessionModel) -> [SessionModel] {
        guard let index = shown.firstIndex(where: { $0.id == lane.id }) else { return [] }
        return Array(shown[(index + 1)...])
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
        a.stage("reading the repository")
        await a.refreshState()
        a.stage("reading git")
        await a.refreshGit()
        await a.refreshTree()
        await a.refreshTrust()
        await a.refreshGatePlan()
        await a.refreshDev()
        a.stage("checking tools and plugins")
        await a.refreshSuggestions()
        await a.refreshTools()
        await refreshWorktrees()
        a.finishedOpening()
        a.offerSetup()
        a.offerReview()
    }
}
