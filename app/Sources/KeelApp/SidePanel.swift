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
    @State private var adopted: [String]?
    @State private var saved: String?
    @State private var busy = false

    var body: some View {
        if let p = model.scan?.profile, p.template != "blank" || !p.hosting.isEmpty {
            profileCard(p)
        }
        actions
        if model.findings.isEmpty {
            Empty(text: "Nothing blocking. Run a scan from the palette for the full report.")
        } else if let plan = model.scan?.plan, !plan.isEmpty {
            ForEach(Array(plan.enumerated()), id: \.element.id) { i, phase in
                RailHeader("\(i + 1). \(phase.title)", trailing: "\(phase.findings.count)")
                Text(phase.why).font(K.F.micro).foregroundStyle(K.C.faint)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, K.S.md).padding(.bottom, K.S.xs)
                ForEach(phase.findings, id: \.self) { id in
                    if let f = model.findings.first(where: { $0.id == id }) { row(f) }
                }
            }
        } else {
            ForEach(model.findings.prefix(40)) { f in row(f) }
        }
    }

    private func profileCard(_ p: Wire.Profile) -> some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            HStack(spacing: K.S.xs) {
                Text("\(model.scan?.score ?? 0)").font(K.F.mono(18, .semibold))
                    .foregroundStyle(score < 60 ? K.C.del : score < 85 ? K.C.warn : K.C.add)
                Text("/100").font(K.F.micro).foregroundStyle(K.C.faint)
                Spacer()
                // The checklist re-runs on every read of the project; this is for the person
                // who just fixed something and wants to see it gone now.
                if model.scanning {
                    ProgressView().controlSize(.mini)
                    Text("scanning…").font(K.F.micro).foregroundStyle(K.C.faint)
                } else {
                    Button { Task { await model.rescan() } } label: {
                        Label("Scan again", systemImage: "arrow.clockwise").font(K.F.micro)
                    }
                    .buttonStyle(QuietButton())
                    .help("Re-reads the repository and re-runs every check. \(model.findings.count) findings now.")
                }
            }
            if p.template != "blank" {
                HStack(spacing: K.S.xs) {
                    Text("Looks like").font(K.F.micro).foregroundStyle(K.C.faint)
                    Text(p.template_title).font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                    if !p.like.isEmpty { Text("like \(p.like)").font(K.F.micro).foregroundStyle(K.C.accent) }
                    Text("\(p.confidence)%").font(K.F.mono(10)).foregroundStyle(K.C.faint)
                }
                .help("From: \(p.signals.joined(separator: ", ")). The template's practices are the target shape.")
            }
            if !p.hosting.isEmpty {
                HStack(spacing: K.S.xs) {
                    Text("Runs on").font(K.F.micro).foregroundStyle(K.C.faint)
                    Text(p.hosting.map(hostName).joined(separator: " + ")).font(K.F.small).foregroundStyle(K.C.text)
                }
            }
            if !p.stack.isEmpty {
                Text(p.stack.joined(separator: " · ")).font(K.F.mono(10)).foregroundStyle(K.C.faint).lineLimit(2)
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
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

    private var actions: some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            // The review a scanner cannot do: read the code as a staff engineer, rank what
            // hurts first, and lay out the PRs. Plan mode, so it changes nothing.
            Button {
                busy = true
                Task { await model.requestReview(); busy = false }
            } label: {
                HStack(spacing: K.S.xs) {
                    Image(systemName: "person.crop.rectangle.stack").font(.system(size: 10))
                    Text(model.lastReview == nil ? "Staff-engineer review" : "Review again").font(K.F.small.weight(.semibold))
                    Spacer()
                    Text(model.lastReview.map { "last \(Self.ago($0)) · runs daily" } ?? "plan mode · reads, changes nothing")
                        .font(K.F.micro).foregroundStyle(K.C.faint)
                }
            }
            .buttonStyle(QuietButton(tone: K.C.accent))
            .disabled(busy || model.running)
            .help("Starts a turn that reviews the repository the way a staff engineer would: what it is, the five things that will hurt first, architecture, security, operability, and the PRs in order.")

            // After a review: keep it, and let it fix the documents it judged.
            if model.lastReview != nil, let last = model.turns.last, !last.text.isEmpty, !model.running {
                HStack(spacing: K.S.xs) {
                    Button {
                        Task { saved = await model.saveReview() }
                    } label: {
                        Label(saved == nil ? "Save review to docs/REVIEW.md" : "Saved \(saved!)", systemImage: saved == nil ? "doc.badge.plus" : "checkmark")
                            .font(K.F.small)
                    }
                    .buttonStyle(QuietButton(tone: saved == nil ? K.C.accent : K.C.add)).disabled(saved != nil)
                    Button {
                        busy = true
                        Task { await model.fixDocs(); busy = false }
                    } label: {
                        Label("Fix CLAUDE.md and AGENTS.md", systemImage: "doc.text.magnifyingglass").font(K.F.small)
                    }
                    .buttonStyle(QuietButton(tone: K.C.accent)).disabled(busy)
                    .help("Starts an editing turn that rewrites the agent instructions from the code, to the standard the review judged them against, then runs the gate.")
                }
            } else if model.findings.contains(where: { $0.id == "agent/thin-instructions" }) {
                Button {
                    busy = true
                    Task { await model.fixDocs(); busy = false }
                } label: {
                    HStack(spacing: K.S.xs) {
                        Image(systemName: "doc.text.magnifyingglass").font(.system(size: 10))
                        Text("Fix CLAUDE.md and AGENTS.md").font(K.F.small.weight(.semibold))
                        Spacer()
                        Text("edits, then runs the gate").font(K.F.micro).foregroundStyle(K.C.faint)
                    }
                }
                .buttonStyle(QuietButton(tone: K.C.accent)).disabled(busy || model.running)
            }

            if model.findings.contains(where: { $0.id == "agent/no-reviewers" }) || adopted != nil {
                Button {
                    busy = true
                    Task { adopted = await model.adoptPractices(); busy = false }
                } label: {
                    HStack(spacing: K.S.xs) {
                        Image(systemName: adopted == nil ? "checkmark.shield" : "checkmark.circle.fill").font(.system(size: 10))
                        Text(adopted == nil ? "Add reviewers and production rules" : "Added \(adopted!.count) files").font(K.F.small.weight(.semibold))
                        Spacer()
                        if adopted == nil { Text("writes 4 files, overwrites none").font(K.F.micro).foregroundStyle(K.C.faint) }
                    }
                }
                .buttonStyle(QuietButton(tone: adopted == nil ? K.C.accent : K.C.add))
                .disabled(busy || adopted != nil)
                .help(".claude/agents/{reviewer,security,reliability}.md and docs/PRODUCTION.md — the same files every Keel template ships. Existing files are left alone.")
            }
        }
        .padding(.horizontal, K.S.sm).padding(.bottom, K.S.xs)
    }

    private func row(_ f: Wire.Finding) -> some View {
        HoverRow {
            VStack(alignment: .leading, spacing: K.S.xxs) {
                Text(f.title).font(K.F.small).foregroundStyle(K.C.text)
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: K.S.xs) {
                    // The severity as a word, not only a colour: a red dot and an amber dot
                    // are the same dot to one person in twelve.
                    Pill(text: f.severity.uppercased(), tone: tone(f.severity))
                    Text(f.id).font(K.F.mono(10)).foregroundStyle(K.C.faint)
                }
            }
        } action: {
            // Every finding carries a fix, so the click is the fix — not a draft in the box.
            Task { await model.fix(f) }
        }
        .disabled(model.running)
        .help(f.detail + "\n\nClick to fix it now.")
    }

    private func tone(_ s: String) -> Pill.Tone {
        switch s {
        case "critical", "high": .bad
        case "medium": .warn
        default: .neutral
        }
    }
}
