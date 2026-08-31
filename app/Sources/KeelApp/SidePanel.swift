import SwiftUI

/// The panel the activity rail opens. One surface, five contents.
struct SidePanel: View {
    let panel: SessionWindow.Panel
    @Bindable var model: SessionModel
    /// Closing the panel. The rail icon toggles it and ⌘⇧E toggles it, and neither is visible.
    var onClose: (() -> Void)?

    var body: some View {
        VStack(spacing: 0) {
            RailHeader(panel.title, trailing: count, onClose: onClose)
            Hairline()
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    // Before the daemon has answered once, an empty list means "not yet", and
                    // showing "nothing here" for it is a lie that lasts half a second and is
                    // believed for longer.
                    if let why = model.loadFailed, !model.loaded {
                        EmptyState(icon: "exclamationmark.triangle",
                                   title: "Keel cannot read this project",
                                   why, actionLabel: "Try again") {
                            Task { await model.refreshState() }
                        }
                    } else if !model.loaded {
                        Loading()
                    } else {
                    switch panel {
                    case .changes: ChangesTreeView(model: model)
                    case .git: GitPanel(model: model)
                    case .files: FileTree(model: model)
                    case .sessions: SessionsPanel(model: model)
                    case .monitors: MonitorsPanel(model: model)
                    case .skills: SkillsPanel(model: model)
                    case .agents: AgentsPanel(model: model)
                    case .mcp: MCPPanel(model: model)
                    case .hooks: HooksPanel(model: model)
                    case .plugins: PluginsPanel(model: model)
                    }
                    }
                }
                .padding(.bottom, K.S.md)
            }
        }
        .background(K.C.surface)
        // The one destructive question in the panels, anchored here for the same reason as
        // the sheets below: a row can be recycled under its own dialog.
        .confirmationDialog("Discard every uncommitted change?", isPresented: $model.confirmingDiscard, titleVisibility: .visible) {
            Button("Discard \(model.changes.count) file\(model.changes.count == 1 ? "" : "s")", role: .destructive) {
                Task { await model.discardAll() }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Modified files go back to the last commit. New files go to the Trash, where they can be recovered. Ignored files such as .env are left alone. The last commit is untouched.")
        }
    }

    private var count: String? {
        let n: Int
        switch panel {
        // What the tree below actually shows. It counted every uncommitted file while the tree
        // listed only what the agent wrote here, so the header disagreed with the list under it.
        case .changes: n = model.editedThisSession.count
        case .git: return nil
        case .sessions: n = model.sessions.count
        case .monitors: n = model.monitors.count
        case .skills: n = model.workspace.skills.count
        case .agents: n = model.workspace.agents.count
        case .mcp: n = model.workspace.mcpServers.count
        case .hooks: n = model.workspace.hooks.count
        case .plugins: n = model.workspace.plugins.count
        case .files: return nil
        }
        return n == 0 ? nil : "\(n)"
    }
}

// MARK: - Sessions

struct SessionsPanel: View {
    let model: SessionModel
    @State private var renaming: String?
    @State private var newTitle = ""
    @State private var query = ""

    private var visible: [Wire.Session] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty else { return model.sessions }
        return model.sessions.filter {
            ($0.title ?? $0.id).localizedCaseInsensitiveContains(q)
            || ($0.cwd ?? "").localizedCaseInsensitiveContains(q)
        }
    }

    var body: some View {
        // Grouped once. It was read again for every row's detail line, which on a parent folder
        // with two hundred sessions is the same grouping done two hundred times.
        let groups = grouped
        if !model.sessions.isEmpty {
            SearchField(prompt: "Find a session", text: $query)
        }
        if model.sessions.isEmpty {
            EmptyState(icon: "clock.arrow.circlepath", title: "No past sessions",
                       "Sessions appear here once a task has run. Current work stays in the task "
                       + "tabs above.")
        } else if visible.isEmpty {
            EmptyState(icon: "magnifyingglass", title: "Nothing matches",
                       "No past session mentions “\(query)”.")
        }
        ForEach(groups, id: \.folder) { group in
            // Only when there is more than one: a single header over every row names the project
            // you are already in, which is the sort of label that makes a panel longer and no
            // clearer. Open a parent folder and there are suddenly twelve, and then it is the
            // only thing that tells them apart.
            if groups.count > 1 {
                HStack(spacing: K.S.xs) {
                    Text(group.folder).sectionLabel().foregroundStyle(K.C.dim)
                    Text("\(group.sessions.count)").font(K.F.micro).foregroundStyle(K.C.faint)
                    Spacer()
                }
                .padding(.horizontal, K.S.md).padding(.top, K.S.sm)
            }
            ForEach(group.sessions) { s in
                PanelRow(name: s.title ?? String(s.id.prefix(8)),
                         detail: detail(s, grouped: groups.count > 1),
                         selected: s.id == model.sessionId) {
                    // Resumed into its own lane, so opening an old session does not evict the one
                    // that is running.
                    Task { await model.lanes?.open(session: s.id) }
                }
                .contextMenu {
                    Button("Rename…") { renaming = s.id; newTitle = s.title ?? "" }
                }
            }
        }
        // Rendered once, outside the loop: an alert per row is an alert per row.
        Color.clear.frame(height: 0)
            .alert("Rename feature", isPresented: Binding(
                get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
                TextField("Name", text: $newTitle)
                Button("Cancel", role: .cancel) { renaming = nil }
                Button("Rename") {
                    if let id = renaming { Task { await model.rename(session: id, to: newTitle) } }
                    renaming = nil
                }
            }
    }

    struct Group: Identifiable {
        var folder: String
        var sessions: [Wire.Session]
        var id: String { folder }
    }

    /// Sessions by the folder they ran in, most recently active folder first.
    ///
    /// Open a parent folder — a monorepo, a client's directory — and History is every session
    /// from every project under it, which was one flat list of two hundred and sixty rows where
    /// nothing said which repository any of them belonged to.
    private var grouped: [Group] {
        var order: [String] = []
        var byFolder: [String: [Wire.Session]] = [:]
        for s in visible {
            let key = folder(s)
            if byFolder[key] == nil { order.append(key) }
            byFolder[key, default: []].append(s)
        }
        // `visible` is already most-recent-first, so first appearance is the folder's own recency.
        return order.map { Group(folder: $0, sessions: byFolder[$0] ?? []) }
    }

    /// Where a session ran, as a label: the path below this project when it is inside it, and the
    /// folder's own name when it is somewhere else.
    private func folder(_ s: Wire.Session) -> String {
        let repo = model.repoPath
        let here = (repo as NSString).lastPathComponent
        guard let cwd = s.cwd, !cwd.isEmpty, cwd != repo else { return here.isEmpty ? "here" : here }
        if cwd.hasPrefix(repo + "/") { return String(cwd.dropFirst(repo.count + 1)) }
        return (cwd as NSString).lastPathComponent
    }

    /// One line under the title: how much was said, when, and — because resuming runs the agent
    /// there — whether it started somewhere other than this folder. It was three monospace
    /// fragments with no separators, reading `42 msg 09:14`.
    private func detail(_ s: Wire.Session, grouped: Bool) -> String {
        var parts = ["\(s.messages) message\(s.messages == 1 ? "" : "s")"]
        if let t = s.lastActive { parts.append(short(t)) }
        // The group header already says where it ran; saying it again on every row is noise.
        if !grouped, let from = s.elsewhere { parts.append("in \(from)/") }
        return parts.joined(separator: " · ")
    }

    /// `2026-08-29T04:12:…` is not a time. The date, or the clock if it is today.
    private func short(_ iso: String) -> String {
        guard iso.count >= 16 else { return iso }
        let day = String(iso.prefix(10))
        // An `ISO8601DateFormatter` allocated per row, in a list that redraws on every state
        // refresh. `.iso8601` is a value type and free to make.
        let today = Date.now.formatted(.iso8601.year().month().day()).prefix(10)
        return day == today ? String(iso.dropFirst(11).prefix(5)) : day
    }
}

// MARK: - Monitors

/// The background commands Keel is running for this conversation.
///
/// They are here rather than in the trace because they are not part of any one turn — that is the
/// whole reason they exist. A turn is one `claude -p` and the CLI kills its own background shells
/// when it ends, so a job that must outlive the turn belongs to the daemon, and the place to see
/// what the daemon is holding is a panel of its own.
struct MonitorsPanel: View {
    let model: SessionModel

    var body: some View {
        if model.monitors.isEmpty {
            EmptyState(icon: "binoculars",
                       title: "Nothing being watched",
                       "When the agent wants to leave a command running — a CI run, a build, a "
                       + "dev server — Keel asks first, then runs it here so it survives the turn "
                       + "and reports back when it finishes.")
        } else {
            ForEach(model.monitors) { job in
                MonitorRow(job: job, model: model)
                Hairline()
            }
        }
    }
}

private struct MonitorRow: View {
    let job: Wire.Job
    let model: SessionModel
    @State private var open = false

    private var took: String {
        let s = Int(job.elapsed)
        return s < 60 ? "\(s)s" : "\(s / 60)m \(s % 60)s"
    }

    /// Running, or how it ended. The exit code is the thing being looked for on a finished job.
    private var status: (text: String, tone: Color) {
        if job.running { return ("running · \(took)", K.C.accent) }
        let code = job.exit ?? -1
        return code == 0 ? ("done in \(took)", K.C.add) : ("exit \(code) after \(took)", K.C.del)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            HStack(spacing: K.S.sm) {
                Text(job.id).font(K.F.micro.monospaced()).foregroundStyle(K.C.faint)
                Text(status.text).font(K.F.micro).foregroundStyle(status.tone)
                Spacer(minLength: 0)
                if job.running {
                    Button("Stop") { model.stopMonitor(job) }
                        .buttonStyle(QuietButton(tone: K.C.del))
                        .help("Interrupt it. The agent is told what it printed before it stopped.")
                }
            }
            Text(job.command)
                .font(K.F.codeSmall).foregroundStyle(K.C.text)
                .lineLimit(open ? nil : 2)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            if open, !job.log.isEmpty {
                Text(job.log.suffix(40).joined(separator: "\n"))
                    .font(K.F.codeSmall).foregroundStyle(K.C.dim)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(K.S.xs)
                    .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .contentShape(Rectangle())
        .asButton { withAnimation(K.M.quick) { open.toggle() } }
    }
}
