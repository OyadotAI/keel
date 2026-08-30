import SwiftUI

/// The panel the activity rail opens. One surface, five contents.
struct SidePanel: View {
    let panel: SessionWindow.Panel
    @Bindable var model: SessionModel

    var body: some View {
        VStack(spacing: 0) {
            RailHeader(panel.title, trailing: count)
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
        ForEach(visible) { s in
            HoverRow(selected: s.id == model.sessionId) {
                VStack(alignment: .leading, spacing: K.S.hair) {
                    Text(s.title ?? String(s.id.prefix(8)))
                        .font(K.F.small.weight(s.id == model.sessionId ? .semibold : .regular))
                        .foregroundStyle(K.C.text)
                        .lineLimit(1)
                    HStack(spacing: K.S.xs) {
                        Text("\(s.messages) msg").font(K.F.codeTiny)
                        if let t = s.lastActive { Text(short(t)).font(K.F.codeTiny) }
                        // Started in the folder above or below: said, because resuming it
                        // runs the agent there.
                        if let from = s.elsewhere {
                            Text("· in \(from)/").font(K.F.codeTiny)
                                .help("Started in \(s.cwd ?? from). Resuming runs the agent there.")
                        }
                    }
                    .foregroundStyle(K.C.faint)
                }
            } action: {
                // Resumed into its own lane, so opening an old session does not evict the one
                // that is running.
                Task { await model.lanes?.open(session: s.id) }
            }
            .contextMenu {
                Button("Rename…") { renaming = s.id; newTitle = s.title ?? "" }
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
