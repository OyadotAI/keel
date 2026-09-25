import AppKit
import SwiftUI

/// Skills, subagents, MCP servers, hooks and plugins — one panel each.
///
/// They were one stacked list, which meant five headings competing in a 256pt rail and no room for
/// any of them to have actions. They are genuinely different things: a skill teaches a task, a
/// subagent is a delegate, an MCP server is a connection, a hook is a shell command that runs
/// without asking, and a plugin is the parcel the first two arrive in. Each gets its own icon.
struct SkillsPanel: View {
    let model: SessionModel
    var actionsInHeader = false

    /// Where a skill came from — the question the flat list could not answer once plugins were in
    /// it. `generated` is a project skill too; it has its own tab because nobody wrote it by hand.
    enum Origin: String, CaseIterable, Identifiable {
        case project, generated, personal, plugins
        var id: Self { self }
        var title: String {
            switch self {
            case .project: "Project"
            case .generated: "Generated"
            case .personal: "Personal"
            case .plugins: "Plugins"
            }
        }
        static func of(_ s: Wire.Named) -> Origin {
            switch s.scope {
            case "plugin": .plugins
            case "project": s.generated == true ? .generated : .project
            default: .personal
            }
        }
    }

    @State private var picked: Origin?
    @State private var query = ""

    private var all: [Wire.Named] { model.workspace.skills }
    private func count(_ o: Origin) -> Int { all.count { Origin.of($0) == o } }
    /// The tab the person chose, else the first with anything in it.
    private var tab: Origin {
        picked ?? Origin.allCases.first { count($0) > 0 } ?? .project
    }
    private var shown: [Wire.Named] {
        let q = query.trimmingCharacters(in: .whitespaces)
        return all.filter {
            Origin.of($0) == tab && (q.isEmpty || $0.name.localizedCaseInsensitiveContains(q)
                || $0.description.localizedCaseInsensitiveContains(q)
                || ($0.plugin ?? "").localizedCaseInsensitiveContains(q))
        }
    }

    var body: some View {
        Group {
            // Skills arrive inside plugins; what the repository is missing is installable here,
            // where the badge that announced it leads.
            Recommended(model: model)
            if all.isEmpty {
                EmptyState(icon: "sparkles", title: "No skills yet",
                      "Add reusable instructions for reviews, deployments, and tools.",
                      actionLabel: actionsInHeader ? nil : "Browse skills") { model.sheet = .skills }
            } else {
                tabs
                search
                if shown.isEmpty { empty } else { rows }
                if !actionsInHeader { PanelFooter("Add skills…") { model.sheet = .skills } }
            }
            if !actionsInHeader { PanelFooter("New skill…", icon: "square.and.pencil") { model.sheet = .newSkill } }
        }
    }

    private var tabs: some View {
        HStack(spacing: K.S.xxs) {
            ForEach(Origin.allCases) { o in
                let on = o == tab
                Button { picked = o } label: {
                    VStack(spacing: K.S.hair) {
                        Text(o.title).font(K.F.micro.weight(on ? .semibold : .regular))
                            .lineLimit(1).minimumScaleFactor(0.8)
                        Text("\(count(o))").font(K.F.codeTiny)
                            .foregroundStyle(on ? K.C.dim : K.C.faint)
                    }
                    .foregroundStyle(on ? K.C.text : K.C.dim)
                    .frame(maxWidth: .infinity).padding(.vertical, K.S.xs)
                    .background(on ? K.C.raised : .clear, in: RoundedRectangle(cornerRadius: K.R.sm))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(o.title), \(count(o))")
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(K.S.xxs)
        .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm + 2))
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
    }

    private var search: some View {
        TextField("Filter skills", text: $query)
            .textFieldStyle(.plain).font(K.F.small)
            .padding(.horizontal, K.S.sm).padding(.vertical, K.S.xs)
            .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            .padding(.horizontal, K.S.md).padding(.bottom, K.S.sm)
    }

    @ViewBuilder private var rows: some View {
        if tab == .plugins {
            // Grouped by plugin: forty skills in one list is the thing that was hard to track.
            let groups = Dictionary(grouping: shown) { $0.plugin ?? "" }
            ForEach(groups.keys.sorted(), id: \.self) { name in
                PluginGroup(name: name, skills: groups[name] ?? [], model: model, row: row)
            }
        } else {
            ForEach(shown) { row($0) }
        }
    }

    private func row(_ s: Wire.Named) -> some View {
        PanelRow(name: s.name, detail: s.description, fromRepo: s.fromRepo,
                 selected: model.inspecting == .skill(s)) {
            model.inspecting = .skill(s)
        }
        // The tabs as destinations, without opening the skill first.
        .contextMenu {
            ForEach(SkillPane.moves(for: s), id: \.label) { m in
                Button(m.label) { Task { _ = await SkillPane.move(model: model, skill: s, m) } }
            }
            if Origin.of(s) != .plugins {
                Divider()
                Button("Move to Trash", role: .destructive) {
                    Task { _ = await SkillPane.remove(model: model, skill: s) }
                }
            }
        }
    }

    /// Each tab says what belongs in it, so an empty one is not a riddle.
    @ViewBuilder private var empty: some View {
        let why: String = if !query.isEmpty { "Nothing in \(tab.title) matches “\(query)”." } else {
            switch tab {
            case .project: "Skills in this repository's .claude/skills travel with the code."
            case .generated: "Skills Keel wrote from a description. They go in this repository."
            case .personal: "Skills in ~/.claude/skills load in every project."
            case .plugins: "No enabled plugin ships skills."
            }
        }
        Text(why).font(K.F.small).foregroundStyle(K.C.dim)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
    }
}

/// One plugin's skills under a heading, open until closed.
private struct PluginGroup<Row: View>: View {
    let name: String
    let skills: [Wire.Named]
    let model: SessionModel
    let row: (Wire.Named) -> Row
    @State private var open = true

    var body: some View {
        PanelSection(title: name, count: skills.count, open: $open) {
            ForEach(skills) { row($0) }
        }
    }
}

struct AgentsPanel: View {
    let model: SessionModel
    var actionsInHeader = false
    @State private var query = ""

    var body: some View {
        Group {
            if !model.workspace.agents.isEmpty {
                SearchField(prompt: "Search subagents", text: $query)
                if !query.isEmpty && !model.workspace.agents.contains(where: { $0.name.localizedCaseInsensitiveContains(query) }) {
                    EmptyState(icon: "magnifyingglass", title: "No matches",
                               "Try another name.", actionLabel: "Clear search") { query = "" }
                }
            }
            if model.workspace.agents.isEmpty {
                EmptyState(icon: "person.2", title: "No subagents",
                      "Create a focused delegate for work that benefits from its own context.",
                      actionLabel: actionsInHeader ? nil : "Create one") { model.sheet = .subagent }
            } else {
                ForEach(model.workspace.agents.filter { query.isEmpty || $0.name.localizedCaseInsensitiveContains(query) }) { a in
                    PanelRow(name: a.name, detail: a.description, fromRepo: a.fromRepo,
                             selected: model.inspecting == .agent(a)) {
                        model.inspecting = .agent(a)
                    }
                }
                if !actionsInHeader { PanelFooter("Create a subagent…") { model.sheet = .subagent } }
            }
        }
    }
}

struct MCPPanel: View {
    let model: SessionModel
    var actionsInHeader = false
    @State private var query = ""

    var body: some View {
        Group {
            if !model.workspace.mcpServers.isEmpty {
                SearchField(prompt: "Search servers", text: $query)
                if !query.isEmpty && !model.workspace.mcpServers.contains(where: { $0.name.localizedCaseInsensitiveContains(query) }) {
                    EmptyState(icon: "magnifyingglass", title: "No matches",
                               "Try another name.", actionLabel: "Clear search") { query = "" }
                }
            }
            if model.workspace.mcpServers.isEmpty {
                EmptyState(icon: "cable.connector", title: "No MCP servers",
                      "Connect external tools such as issue trackers, databases, and browsers.",
                      actionLabel: actionsInHeader ? nil : "Add a server…") { model.sheet = .mcp }
            } else {
                ForEach(model.workspace.mcpServers.filter { query.isEmpty || $0.name.localizedCaseInsensitiveContains(query) }) { s in
                    PanelRow(name: s.name, detail: s.description, fromRepo: s.fromRepo,
                             selected: model.inspecting == .mcp(s)) {
                        model.inspecting = .mcp(s)
                    }
                }
                if !actionsInHeader { PanelFooter("Add a server…") { model.sheet = .mcp } }
            }
        }
    }
}

/// Hooks get the loudest treatment, because a hook is a shell command that runs on the machine of
/// whoever opens the repository — the one thing here that is dangerous by construction.
struct HooksPanel: View {
    let model: SessionModel

    @State private var showRepo = true
    @State private var showMine = true

    private var fromRepo: [Wire.Hook] { model.workspace.hooks.filter(\.fromRepo) }
    private var mine: [Wire.Hook] { model.workspace.hooks.filter { !$0.fromRepo } }

    var body: some View {
            if model.workspace.hooks.isEmpty {
            // The one empty state with nothing to offer: hooks are written into the repository or
            // into `~/.claude`, and Keel deliberately does not write either.
            EmptyState(icon: "bolt.horizontal", title: "No hooks",
                       "Hooks run local commands around agent actions. Repository hooks stay "
                       + "quarantined until you trust the project.")
        } else {
            if !fromRepo.isEmpty {
                PanelSection(title: "From this repository", count: fromRepo.count,
                             open: $showRepo) {
                    ForEach(fromRepo) { h in row(h) }
                }
            }
            if !mine.isEmpty {
                PanelSection(title: "Yours", count: mine.count, open: $showMine) {
                    ForEach(mine) { h in row(h) }
                }
            }
        }
    }

    private func row(_ h: Wire.Hook) -> some View {
        PanelRow(name: h.event, detail: h.command, fromRepo: h.fromRepo,
                 selected: model.inspecting == .hook(h), code: true) {
            model.inspecting = .hook(h)
        }
    }
}

struct PluginsPanel: View {
    let model: SessionModel
    var actionsInHeader = false
    @State private var query = ""

    var body: some View {
        Group {
            if !model.workspace.plugins.isEmpty {
                SearchField(prompt: "Search plugins", text: $query)
                if !query.isEmpty && !model.workspace.plugins.contains(where: { $0.name.localizedCaseInsensitiveContains(query) }) {
                    EmptyState(icon: "magnifyingglass", title: "No matches",
                               "Try another name.", actionLabel: "Clear search") { query = "" }
                }
            }
            Recommended(model: model)
            if model.workspace.plugins.isEmpty {
                EmptyState(icon: "puzzlepiece.extension", title: "No plugins",
                      "Install bundles of skills, subagents, and commands.",
                      actionLabel: actionsInHeader ? nil : "Browse plugins") { model.sheet = .skills }
            } else {
                ForEach(model.workspace.plugins.filter { query.isEmpty || $0.name.localizedCaseInsensitiveContains(query) }) { p in
                    PanelRow(name: p.name,
                             detail: p.enabled ? p.marketplace : "disabled · \(p.marketplace)",
                             dimmed: !p.enabled,
                             selected: model.inspecting == .plugin(p)) {
                        model.inspecting = .plugin(p)
                    }
                }
                if !actionsInHeader { PanelFooter("Browse plugins…") { model.sheet = .skills } }
            }
        }
    }
}

// MARK: - Shared pieces

struct Recommended: View {
    let model: SessionModel

    var body: some View {
        if !model.missingSuggestions.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: K.S.xs) {
                    Image(systemName: "exclamationmark.triangle.fill").font(K.F.tiny)
                        .foregroundStyle(K.C.warn)
                    Text("Recommended for this repository")
                        .sectionLabel()
                        .foregroundStyle(K.C.warn)
                }
                .padding(.horizontal, K.S.md).padding(.top, K.S.md).padding(.bottom, K.S.xs)

                ForEach(model.missingSuggestions) { e in
                    VStack(alignment: .leading, spacing: K.S.xxs) {
                        HStack(spacing: K.S.sm) {
                            Text(e.name).font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                            Spacer()
                            Button(model.installing == e.id ? "Installing…" : "Install") {
                                Task { await model.installPlugin(e) }
                            }
                            .buttonStyle(QuietButton(tone: K.C.accent))
                            .disabled(model.installing != nil)
                        }
                        Text(e.reason ?? e.description ?? "")
                            .font(K.F.micro).foregroundStyle(K.C.dim)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(.horizontal, K.S.md).padding(.vertical, K.S.xs)
                }
                if !model.installLog.isEmpty {
                    Text(model.installLog.suffix(400))
                        .font(K.F.codeSmall).foregroundStyle(K.C.faint)
                        .lineLimit(4)
                        .padding(.horizontal, K.S.md).padding(.vertical, K.S.xs)
                }
                Hairline().padding(.top, K.S.xs)
            }
            .background(K.C.warn.wash)
        }
    }
}

/// An empty panel that says what the thing is for, rather than a heading with nothing under it.
