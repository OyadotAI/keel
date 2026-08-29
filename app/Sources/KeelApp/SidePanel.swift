import SwiftUI

/// The panel the activity rail opens. One surface, five contents.
struct SidePanel: View {
    let panel: SessionWindow.Panel
    @Bindable var model: SessionModel

    var body: some View {
        VStack(spacing: 0) {
            RailHeader(panel.title, trailing: count)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    // Before the daemon has answered once, an empty list means "not yet", and
                    // showing "nothing here" for it is a lie that lasts half a second and is
                    // believed for longer.
                    if !model.loaded {
                        Empty(text: "Reading…")
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
        // Every sheet the panels open, anchored here on a view that is never recycled.
        .sheet(item: $model.sheet) { which in
            switch which {
            case .pr:
                PullRequest(model: model) { model.sheet = nil }
            case .skills:
                SkillCatalog(client: model.client) {
                    model.sheet = nil
                    Task { await model.refreshState(); await model.refreshSuggestions() }
                }
            case .subagent:
                NewSubagent(client: model.client) {
                    model.sheet = nil
                    Task { await model.refreshState() }
                }
            case .mcp:
                AddMCP(client: model.client) {
                    model.sheet = nil
                    Task { await model.refreshState() }
                }
            case .setup:
                SetupSheet(model: model) { model.sheet = nil }
            }
        }
    }

    private var count: String? {
        let n: Int
        switch panel {
        case .changes: n = model.changes.count
        case .git: return model.branch
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

private struct Empty: View {
    let text: String
    var body: some View {
        Text(text)
            .font(K.F.small).foregroundStyle(K.C.faint)
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
    }
}

// MARK: - Sessions

struct SessionsPanel: View {
    let model: SessionModel
    @State private var renaming: String?
    @State private var newTitle = ""

    var body: some View {
        if model.sessions.isEmpty { Empty(text: "No sessions in this project yet.") }
        ForEach(model.sessions) { s in
            HoverRow(selected: s.id == model.sessionId) {
                VStack(alignment: .leading, spacing: 1) {
                    Text(s.title ?? String(s.id.prefix(8)))
                        .font(K.F.small.weight(s.id == model.sessionId ? .semibold : .regular))
                        .foregroundStyle(K.C.text)
                        .lineLimit(1)
                    HStack(spacing: K.S.xs) {
                        Text("\(s.messages) msg").font(K.F.mono(10))
                        if let t = s.lastActive { Text(short(t)).font(K.F.mono(10)) }
                        // Started in the folder above or below: said, because resuming it
                        // runs the agent there.
                        if let from = s.elsewhere {
                            Text("· in \(from)/").font(K.F.mono(10))
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
            .alert("Rename session", isPresented: Binding(
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
        let today = ISO8601DateFormatter().string(from: .now).prefix(10)
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
    @State private var open: Set<String> = []

    var body: some View {
        card
        if model.findings.isEmpty {
            Empty(text: "Nothing outstanding. The checks re-run every time the project is read.")
        } else if let plan = model.scan?.plan, !plan.isEmpty {
            ForEach(Array(plan.enumerated()), id: \.element.id) { i, phase in
                phaseRows(i, phase, expanded: open.contains(phase.id) || (open.isEmpty && i == 0))
            }
        } else {
            ForEach(model.findings.prefix(40)) { f in row(f) }
        }
    }

    // MARK: The card: the score, what it is, where it runs, and the one thing to do next.

    private var card: some View {
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
                        Image(systemName: "arrow.clockwise").font(.system(size: 10))
                    }
                    .buttonStyle(QuietButton()).help("Scan again — re-reads the repository and re-runs every check.")
                }
            }
            if let p = model.scan?.profile {
                VStack(alignment: .leading, spacing: 2) {
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
        .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
        .padding(.horizontal, K.S.sm).padding(.vertical, K.S.sm)
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
                HStack(spacing: K.S.xs) {
                    Image(systemName: "person.crop.rectangle.stack").font(.system(size: 10, weight: .semibold))
                    Text(model.lastReview == nil ? "Staff-engineer review" : "Review again")
                        .font(K.F.small.weight(.semibold))
                }
                .foregroundStyle(.white)
                .padding(.horizontal, K.S.sm).padding(.vertical, 5)
                .background(K.C.accent, in: RoundedRectangle(cornerRadius: K.R.sm))
            }
            .buttonStyle(.plain)
            .disabled(busy || (model.reviewLane?.running ?? false))
            .help("Runs in its own lane, beside this one. Reads the code the way a staff engineer would — hot path, threat model, tests, pipeline, docs — and ranks what hurts first. Plan mode: changes nothing. Runs on its own once a day.")
            if reviewed {
                Menu {
                    Button(saved == nil ? "Save as the contract" : "Saved \(saved!)") { Task { saved = await model.saveReview() } }
                        .disabled(saved != nil)
                    Button("Fix CLAUDE.md and AGENTS.md") { busy = true; Task { await model.fixDocs(); busy = false } }
                } label: {
                    Image(systemName: "ellipsis").font(.system(size: 11, weight: .semibold))
                        .frame(width: 22, height: 22)
                }
                .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                .help("Keep the review as .claude/agents/contract.md (yours to correct; every turn reads it), or rewrite the agent instructions from the code.")
            }
            Spacer()
            if let last = model.lastReview {
                Text(Self.ago(last)).font(K.F.micro).foregroundStyle(K.C.faint)
            }
        }
    }

    // MARK: Phases: a heading with a count; open one to see its findings, click one to fix it.

    @ViewBuilder
    private func phaseRows(_ i: Int, _ phase: Wire.Phase, expanded: Bool) -> some View {
        HoverRow {
            HStack(spacing: K.S.xs) {
                Image(systemName: expanded ? "chevron.down" : "chevron.right")
                    .font(.system(size: 10, weight: .semibold)).foregroundStyle(K.C.faint).frame(width: 10)
                Text("\(i + 1). \(phase.title)").font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                Spacer()
                Text("\(phase.findings.count)").font(K.F.mono(10)).foregroundStyle(K.C.faint)
            }
        } action: {
            withAnimation(K.M.quick) {
                if open.isEmpty { open = Set((model.scan?.plan ?? []).prefix(1).map(\.id)) }
                if open.contains(phase.id) { open.remove(phase.id) } else { open.insert(phase.id) }
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

    private func row(_ f: Wire.Finding) -> some View {
        HoverRow {
            HStack(alignment: .top, spacing: K.S.sm) {
                Pill(text: String(f.severity.prefix(4)).uppercased(), tone: tone(f.severity))
                    .padding(.top, 1)
                Text(f.title).font(K.F.small).foregroundStyle(K.C.text)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }
            .padding(.leading, K.S.sm)
        } action: {
            // Every finding carries a fix, so the click is the fix — not a draft in the box.
            Task { await model.fix(f) }
        }
        .disabled(model.running)
        .help(f.detail + "\n\n" + f.id + " · click to fix it now.")
    }

    private func tone(_ s: String) -> Pill.Tone {
        switch s {
        case "critical", "high": .bad
        case "medium": .warn
        default: .neutral
        }
    }
}
