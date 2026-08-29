import SwiftUI

/// The panel the activity rail opens. One surface, five contents.
struct SidePanel: View {
    let panel: SessionWindow.Panel
    let model: SessionModel

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
    }

    private var count: String? {
        let n: Int
        switch panel {
        case .changes: n = model.changes.count
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

struct ReadinessPanel: View {
    let model: SessionModel

    var body: some View {
        if model.findings.isEmpty {
            Empty(text: "Nothing blocking. Run a scan from the palette for the full report.")
        }
        ForEach(model.findings.prefix(30)) { f in
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
                // Every finding carries a fix, so the useful click is "ask for it".
                model.prompt = "Fix this readiness finding: \(f.title)\n\n\(f.detail)"
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
