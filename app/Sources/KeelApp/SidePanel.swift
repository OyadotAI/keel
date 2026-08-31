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
                    case .readiness: ReadinessPanel(model: model)
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
        case .readiness: n = model.findings.count
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

// MARK: - Readiness

/// Not a list of complaints: what the repository is, where it runs, and the road from here
/// to production in order — with the two actions that start the road, and the review a
/// person would give beyond what a scanner can see.
struct ReadinessPanel: View {
    let model: SessionModel
    @State private var saved: String?
    @State private var busy = false
    @State private var opened: Set<String> = []
    @State private var openFinding: String?

    var body: some View {
        card
        if model.scan == nil {
            // Nothing below the card until there is something to say — the card is already the
            // whole state, and a second empty state under it read as a panel that had broken.
            EmptyView()
        } else if model.findings.isEmpty {
            EmptyState(icon: "checkmark.seal", title: "Nothing outstanding",
                       "Keel checks again whenever the repository changes.")
        } else if let plan = model.scan?.plan, !plan.isEmpty {
            if let next = model.findings.first { nextAction(next) }
            ForEach(Array(plan.enumerated()), id: \.element.id) { i, phase in
                phaseRows(i, phase, expanded: opened.contains(phase.id))
            }
        } else {
            ForEach(model.findings.prefix(40)) { f in row(f) }
        }
        ignoredRow
    }

    private func nextAction(_ finding: Wire.Finding) -> some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            Text("FIX NEXT").sectionLabel()
                .foregroundStyle(K.C.warn)
            Text(finding.title).font(K.F.body.weight(.semibold)).foregroundStyle(K.C.text)
                .fixedSize(horizontal: false, vertical: true)
            Text(finding.detail).font(K.F.micro).foregroundStyle(K.C.dim).lineLimit(3)
            Button("Fix this") { Task { await model.fix(finding) } }
                .buttonStyle(FilledButton()).disabled(model.running)
        }
        .padding(K.S.md)
        .background(K.C.warn.wash, in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.warn.opacity(0.35), lineWidth: 1))
        .padding(.horizontal, K.S.sm).padding(.vertical, K.S.sm)
    }

    // MARK: The card: the score, what it is, where it runs, and the one thing to do next.

    private var card: some View {
        Group {
            if model.scan == nil {
                unscanned
            } else {
                scanned
            }
        }
    }

    /// Never scanned, or scanned and it failed. The second case used to be indistinguishable from
    /// the first: `rescan` went through a `try?`, so a scan that could not run left the panel
    /// saying "not checked" with a Scan button that appeared to do nothing.
    @ViewBuilder
    private var unscanned: some View {
        if model.scanning {
            Loading("Scanning the repository…")
        } else if let why = model.loadFailed {
            EmptyState(icon: "exclamationmark.triangle", title: "The scan did not run", why,
                       actionLabel: "Try again") { Task { await model.rescan() } }
        } else {
            EmptyState(icon: "checkmark.shield", title: "Readiness not checked",
                       "Keel reads the repository for release blockers — no network, nothing sent.",
                       actionLabel: "Scan") { Task { await model.rescan() } }
        }
    }

    private var scanned: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            HStack(alignment: .firstTextBaseline, spacing: K.S.xs) {
                Text("\(score)").font(K.F.mono(22, .semibold))
                    .foregroundStyle(score < 60 ? K.C.del : score < 85 ? K.C.warn : K.C.add)
                Text("/100").font(K.F.micro).foregroundStyle(K.C.faint)
                Spacer()
                if model.scanning {
                    ProgressView().controlSize(.mini)
                } else {
                    Button { Task { await model.rescan() } } label: {
                        Image(systemName: "arrow.clockwise").font(K.F.tiny)
                    }
                    .buttonStyle(QuietButton())
                    .hint("Scan again — re-reads the repository and re-runs every check.")
                }
            }
            if let p = model.scan?.profile {
                VStack(alignment: .leading, spacing: K.S.xxs) {
                    if p.template != "blank" {
                        HStack(spacing: K.S.xs) {
                            Text(p.template_title).font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                            if !p.like.isEmpty { Text("like \(p.like)").font(K.F.micro).foregroundStyle(K.C.accent) }
                        }
                        .help("Closest template, \(p.confidence)% from: \(p.signals.joined(separator: ", ")). Its practices are the target shape.")
                    }
                    if !p.hosting.isEmpty || !p.stack.isEmpty {
                        Text(whereLine(p)).font(K.F.micro).foregroundStyle(K.C.faint).lineLimit(2)
                    }
                }
            }
            primary
        }
        .padding(K.S.md)
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.lg)
        .overlay(alignment: .bottom) { Hairline() }
    }

    /// One button. What it does depends on where the project is: review first; after a
    /// review, keep it as the contract and fix the docs it judged.
    @ViewBuilder
    private var primary: some View {
        let lane = model.reviewLane
        let reviewed = model.lastReview != nil && !(lane?.turns.last?.text.isEmpty ?? true) && !(lane?.running ?? true)
        HStack(spacing: K.S.xs) {
            Button {
                busy = true
                Task { await model.requestReview(); busy = false }
            } label: {
                Label(model.lastReview == nil ? "Staff-engineer review" : "Review again",
                      systemImage: "person.crop.rectangle.stack")
            }
            .buttonStyle(FilledButton())
            .disabled(busy || (model.reviewLane?.running ?? false))
            .help("Runs in its own lane, beside this one. Reads the code the way a staff engineer would — hot path, threat model, tests, pipeline, docs — and ranks what hurts first. Plan mode: changes nothing. Runs on its own once a day.")
            if reviewed {
                Menu {
                    Button(saved == nil ? "Save as the contract" : "Saved \(saved!)") { Task { saved = await model.saveReview() } }
                        .disabled(saved != nil)
                    Button("Fix CLAUDE.md and AGENTS.md") { busy = true; Task { await model.fixDocs(); busy = false } }
                } label: {
                    Image(systemName: "ellipsis").font(K.F.micro.weight(.semibold))
                        .frame(width: 22, height: 22)
                }
                .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                .hint("Keep the review as .claude/agents/contract.md (yours to correct; every turn reads "
                      + "it), or rewrite the agent instructions from the code.")
            }
            Spacer()
            if let last = model.lastReview {
                Text(Self.ago(last)).font(K.F.micro).foregroundStyle(K.C.faint)
            }
        }
        // Where the background review is: running, or ready to open.
        if let lane = model.reviewLane, lane !== model {
            HStack(spacing: K.S.xs) {
                if lane.running {
                    ProgressView().controlSize(.mini)
                    Text("Reviewing in the background…").font(K.F.micro).foregroundStyle(K.C.faint)
                } else if !(lane.turns.last?.text.isEmpty ?? true) {
                    Image(systemName: "doc.text").font(K.F.tiny).foregroundStyle(K.C.accent)
                    Button("Open the review") { model.openReview() }
                        .buttonStyle(.plain).font(K.F.small.weight(.semibold)).foregroundStyle(K.C.accent)
                    Text("· opens as a tab").font(K.F.micro).foregroundStyle(K.C.faint)
                }
                Spacer()
            }
        }
    }

    // MARK: Phases: a heading with a count; open one to see its findings, click one to fix it.

    @ViewBuilder
    private func phaseRows(_ i: Int, _ phase: Wire.Phase, expanded: Bool) -> some View {
        HoverRow {
            HStack(spacing: K.S.xs) {
                Image(systemName: expanded ? "chevron.down" : "chevron.right")
                    .font(K.F.tiny.weight(.semibold)).foregroundStyle(K.C.faint).frame(width: 10)
                Text("\(i + 1). \(phase.title)").font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                Spacer()
                Text("\(phase.findings.count)").font(K.F.codeTiny).foregroundStyle(K.C.faint)
            }
        } action: {
            withAnimation(K.M.quick) {
                if opened.contains(phase.id) { opened.remove(phase.id) } else { opened.insert(phase.id) }
            }
        }
        .help(phase.why)
        if expanded {
            ForEach(phase.findings, id: \.self) { id in
                if let f = model.findings.first(where: { $0.id == id }) { row(f) }
            }
        }
    }

    /// "on Vercel + Supabase · next · postgres · zod"
    private func whereLine(_ p: Wire.Profile) -> String {
        var parts: [String] = []
        if !p.hosting.isEmpty { parts.append("on " + p.hosting.map(hostName).joined(separator: " + ")) }
        let stack = p.stack.prefix(5).joined(separator: " · ")
        if !stack.isEmpty { parts.append(stack) }
        return parts.joined(separator: " · ")
    }

    private var score: Int { model.scan?.score ?? 0 }
    static func ago(_ d: Date) -> String {
        let s = Int(Date().timeIntervalSince(d))
        return s < 3600 ? "\(max(1, s / 60)) min ago" : s < 86_400 ? "\(s / 3600) h ago" : "\(s / 86_400) d ago"
    }
    private func hostName(_ h: String) -> String {
        switch h {
        case "Gcp": "Google Cloud"
        case "Aws": "AWS"
        case "Fly": "Fly.io"
        default: h
        }
    }

    /// A finding opens where it is: what it is, why it matters, and the two answers — fix it
    /// now, or set it aside. Clicking used to start a turn with no warning and no way back.
    @ViewBuilder
    private func row(_ f: Wire.Finding) -> some View {
        let isOpen = openFinding == f.id
        VStack(alignment: .leading, spacing: 0) {
            HoverRow(selected: isOpen) {
                HStack(alignment: .top, spacing: K.S.sm) {
                    Image(systemName: isOpen ? "chevron.down" : "chevron.right")
                        .font(K.F.tiny.weight(.semibold)).foregroundStyle(K.C.faint)
                        .frame(width: 10).padding(.top, K.S.xxs)
                    Pill(text: String(f.severity.prefix(4)).uppercased(), tone: tone(f.severity))
                        .padding(.top, K.S.hair)
                    Text(f.title).font(K.F.small).foregroundStyle(K.C.text)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                }
                .padding(.leading, K.S.sm)
            } action: {
                withAnimation(K.M.quick) { openFinding = isOpen ? nil : f.id }
            }

            if isOpen {
                VStack(alignment: .leading, spacing: K.S.sm) {
                    Text(f.detail).font(K.F.small).foregroundStyle(K.C.dim)
                        .fixedSize(horizontal: false, vertical: true)
                    if let path = f.path, !path.isEmpty {
                        Text(path).font(K.F.codeTiny).foregroundStyle(K.C.faint).lineLimit(1)
                            .truncationMode(.head)
                    }
                    HStack(spacing: K.S.xs) {
                        Button("Fix it") { openFinding = nil; Task { await model.fix(f) } }
                            .buttonStyle(FilledButton())
                            .disabled(model.running)
                            .help("Starts a turn that fixes this, in a mode that may edit, then runs the gate.")
                        Button("Ignore") { openFinding = nil; Task { await model.ignore(f.id, true) } }
                            .buttonStyle(QuietButton())
                            .help("Sets it aside in .keel/ignored.json. The score is unchanged and `keel scan` still reports it.")
                        Spacer()
                        Text(f.id).font(K.F.codeTiny).foregroundStyle(K.C.faint)
                            .textSelection(.enabled)
                    }
                }
                .padding(.horizontal, K.S.md).padding(.top, K.S.xs).padding(.bottom, K.S.sm)
                .padding(.leading, K.S.sm)
                .transition(.opacity)
            }
        }
    }

    /// What was set aside, so it is not lost — one line, and a way back.
    @ViewBuilder
    private var ignoredRow: some View {
        let ids = model.scan?.ignored ?? []
        if !ids.isEmpty {
            RailHeader("Ignored", trailing: "\(ids.count)")
            ForEach(ids, id: \.self) { id in
                HoverRow {
                    HStack(spacing: K.S.sm) {
                        Image(systemName: "eye.slash").font(K.F.tiny).foregroundStyle(K.C.faint)
                        Text(id).font(K.F.codeSmall).foregroundStyle(K.C.dim).lineLimit(1)
                        Spacer()
                        Text("restore").font(K.F.micro).foregroundStyle(K.C.accent)
                    }
                    .padding(.leading, K.S.sm)
                } action: {
                    Task { await model.ignore(id, false) }
                }
                .hint("Bring this finding back into the report.")
            }
        }
    }

    private func tone(_ s: String) -> Pill.Tone {
        switch s {
        case "critical", "high": .bad
        case "medium": .warn
        default: .neutral
        }
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
