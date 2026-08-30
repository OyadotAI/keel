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

    var body: some View {
        Group {
            // Skills arrive inside plugins, so a gap shows up there; this is the pointer.
            if !model.missingSuggestions.isEmpty {
                HStack(spacing: K.S.xs) {
                    Image(systemName: "exclamationmark.triangle.fill").font(K.F.tiny)
                        .foregroundStyle(K.C.warn)
                    Text("\(model.missingSuggestions.count) recommended plugin\(model.missingSuggestions.count == 1 ? "" : "s") "
                         + "would add skills for this repository — see Plugins.")
                        .font(K.F.micro).foregroundStyle(K.C.dim)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
            }
            if model.workspace.skills.isEmpty {
                EmptyState(icon: "sparkles", title: "No skills yet",
                      "Add reusable instructions for reviews, deployments, and tools.",
                      actionLabel: "Browse skills") { model.sheet = .skills }
            } else {
                RailHeader("Available skills", trailing: "\(model.workspace.skills.count)")
                ForEach(model.workspace.skills) { s in
                    ItemRow(name: s.name, detail: s.description, fromRepo: s.fromRepo,
                            selected: model.inspecting == .skill(s)) {
                        model.inspecting = .skill(s)
                    }
                }
                PanelAction("Add skills…", icon: "plus.circle") { model.sheet = .skills }
            }
        }
    }
}

struct AgentsPanel: View {
    let model: SessionModel

    var body: some View {
        Group {
            if model.workspace.agents.isEmpty {
                EmptyState(icon: "person.2", title: "No subagents",
                      "Create a focused delegate for work that benefits from its own context.",
                      actionLabel: "Create one") { model.sheet = .subagent }
            } else {
                ForEach(model.workspace.agents) { a in
                    ItemRow(name: a.name, detail: a.description, fromRepo: a.fromRepo,
                            selected: model.inspecting == .agent(a)) {
                        model.inspecting = .agent(a)
                    }
                }
                PanelAction("Create a subagent…", icon: "plus.circle") { model.sheet = .subagent }
            }
        }
    }
}

struct MCPPanel: View {
    let model: SessionModel

    var body: some View {
        Group {
            if model.workspace.mcpServers.isEmpty {
                EmptyState(icon: "cable.connector", title: "No MCP servers",
                      "Connect external tools such as issue trackers, databases, and browsers.",
                      actionLabel: "Add a server…") { model.sheet = .mcp }
            } else {
                ForEach(model.workspace.mcpServers) { s in
                    ItemRow(name: s.name, detail: s.description, fromRepo: s.fromRepo,
                            selected: model.inspecting == .mcp(s)) {
                        model.inspecting = .mcp(s)
                    }
                }
                PanelAction("Add a server…", icon: "plus.circle") { model.sheet = .mcp }
            }
        }
    }
}

/// Hooks get the loudest treatment, because a hook is a shell command that runs on the machine of
/// whoever opens the repository — the one thing here that is dangerous by construction.
struct HooksPanel: View {
    let model: SessionModel

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
                RailHeader("From this repository", trailing: "\(fromRepo.count)")
                ForEach(fromRepo) { h in row(h) }
            }
            if !mine.isEmpty {
                RailHeader("Your hooks", trailing: "\(mine.count)")
                ForEach(mine) { h in row(h) }
            }
        }
    }

    private func row(_ h: Wire.Hook) -> some View {
        ItemRow(name: h.event, detail: h.command, fromRepo: h.fromRepo,
                selected: model.inspecting == .hook(h)) {
            model.inspecting = .hook(h)
        }
    }
}

struct PluginsPanel: View {
    let model: SessionModel

    var body: some View {
        Group {
            Recommended(model: model)
            if model.workspace.plugins.isEmpty {
                EmptyState(icon: "puzzlepiece.extension", title: "No plugins",
                      "Install bundles of skills, subagents, and commands.",
                      actionLabel: "Browse plugins") { model.sheet = .skills }
            } else {
                RailHeader("Installed plugins", trailing: "\(model.workspace.plugins.count)")
                ForEach(model.workspace.plugins) { p in
                    ItemRow(name: p.name,
                            detail: p.enabled ? p.marketplace : "disabled · \(p.marketplace)",
                            fromRepo: false,
                            dimmed: !p.enabled,
                            selected: model.inspecting == .plugin(p)) {
                        model.inspecting = .plugin(p)
                    }
                }
                PanelAction("Browse plugins…", icon: "plus.circle") { model.sheet = .skills }
            }
        }
    }
}

// MARK: - Shared pieces

struct ItemRow: View {
    let name: String
    let detail: String
    var fromRepo = false
    var dimmed = false
    var selected = false
    let action: () -> Void

    var body: some View {
        HoverRow(selected: selected) {
            VStack(alignment: .leading, spacing: K.S.hair) {
                HStack(spacing: K.S.xs) {
                    Text(name)
                        .font(K.F.small.weight(selected ? .semibold : .regular))
                        .foregroundStyle(dimmed ? K.C.faint : K.C.text)
                        .lineLimit(1)
                    if fromRepo {
                        Text("repo")
                            .font(K.F.tiny.weight(.semibold))
                            .padding(.horizontal, K.S.tight).padding(.vertical, K.S.hair)
                            .background(K.C.warn.opacity(0.2),
                                        in: RoundedRectangle(cornerRadius: 2))
                            .foregroundStyle(K.C.warn)
                    }
                    Spacer(minLength: 0)
                }
                if !detail.isEmpty {
                    Text(detail)
                        .font(K.F.codeTiny).foregroundStyle(K.C.faint)
                        .lineLimit(1).truncationMode(.tail)
                }
            }
        } action: {
            action()
        }
        .help(detail.isEmpty ? name : detail)
    }
}

struct PanelAction: View {
    let title: String
    let icon: String
    let action: () -> Void

    init(_ title: String, icon: String, action: @escaping () -> Void) {
        self.title = title
        self.icon = icon
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            HStack(spacing: K.S.snug) {
                Image(systemName: icon).font(K.F.tiny)
                Text(title)
                Spacer()
            }
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .font(K.F.small).foregroundStyle(K.C.accent)
    }
}

/// What this repository is missing, at the top of the panel, with the button beside the reason.
///
/// It was a dialog behind a badge. A recommendation nobody opens does nothing, and a badge
/// that stayed lit after the install did the opposite of what a badge is for.
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
