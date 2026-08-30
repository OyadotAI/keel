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
                ForEach(model.workspace.skills) { s in
                    PanelRow(name: s.name, detail: s.description, fromRepo: s.fromRepo,
                             selected: model.inspecting == .skill(s)) {
                        model.inspecting = .skill(s)
                    }
                }
                PanelFooter("Add skills…") { model.sheet = .skills }
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
                    PanelRow(name: a.name, detail: a.description, fromRepo: a.fromRepo,
                             selected: model.inspecting == .agent(a)) {
                        model.inspecting = .agent(a)
                    }
                }
                PanelFooter("Create a subagent…") { model.sheet = .subagent }
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
                    PanelRow(name: s.name, detail: s.description, fromRepo: s.fromRepo,
                             selected: model.inspecting == .mcp(s)) {
                        model.inspecting = .mcp(s)
                    }
                }
                PanelFooter("Add a server…") { model.sheet = .mcp }
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

    var body: some View {
        Group {
            Recommended(model: model)
            if model.workspace.plugins.isEmpty {
                EmptyState(icon: "puzzlepiece.extension", title: "No plugins",
                      "Install bundles of skills, subagents, and commands.",
                      actionLabel: "Browse plugins") { model.sheet = .skills }
            } else {
                ForEach(model.workspace.plugins) { p in
                    PanelRow(name: p.name,
                             detail: p.enabled ? p.marketplace : "disabled · \(p.marketplace)",
                             dimmed: !p.enabled,
                             selected: model.inspecting == .plugin(p)) {
                        model.inspecting = .plugin(p)
                    }
                }
                PanelFooter("Browse plugins…") { model.sheet = .skills }
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
