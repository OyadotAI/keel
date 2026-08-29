import AppKit
import SwiftUI

/// What a skill, subagent, MCP server, hook or plugin actually is — and what you can do with it.
///
/// A row you cannot open tells you a name you already knew. Each of these has a scope that decides
/// whether it is yours or the repository's, a place it lives, and a small set of honest actions.
struct Inspector: View {
    let model: SessionModel
    let target: SessionModel.Inspect

    @State private var log = ""
    @State private var busy = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Hairline()
            ScrollView {
                VStack(alignment: .leading, spacing: K.S.lg) {
                    detail
                    if !log.isEmpty {
                        Text(log)
                            .font(K.F.codeSmall).foregroundStyle(K.C.dim)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(K.S.sm)
                            .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                    }
                }
                .padding(K.S.lg)
            }
        }
        .background(K.C.bg)
    }

    private var header: some View {
        HStack(spacing: K.S.sm) {
            Image(systemName: icon).font(.system(size: 11)).foregroundStyle(K.C.faint)
            Text(name).font(K.F.body.weight(.medium)).foregroundStyle(K.C.text)
                .textSelection(.enabled)
            if fromRepo { Pill(text: "FROM THIS REPO", tone: .warn) }
            Spacer()
            CloseButton { model.inspecting = nil }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .background(K.C.surface)
    }

    // MARK: Per-kind detail

    @ViewBuilder
    private var detail: some View {
        switch target {
        case .skill(let s):
            field("What it does", s.description.isEmpty
                  ? "No description. The agent decides whether to use a skill by reading this, so "
                    + "one without a description will rarely be used."
                  : s.description)
            field("Scope", s.scope == "project"
                  ? "This repository. It came with the checkout."
                  : "You. Available in every project and in the terminal.")
            if let path = s.path { pathField(path) }
            actions {
                if let path = s.path { revealButton(path) }
            }

        case .agent(let a):
            field("When the main agent delegates to it", a.description.isEmpty
                  ? "No description — so the main agent has nothing to decide on."
                  : a.description)
            field("Scope", a.scope == "project" ? "This repository" : "You, everywhere")
            if let path = a.path { pathField(path) }
            actions {
                if let path = a.path { revealButton(path) }
            }

        case .mcp(let s):
            field("What it is", "An MCP server: tools the agent can call that live outside this "
                  + "machine's filesystem.")
            field("Scope", s.fromRepo
                  ? "This repository — configured by whoever wrote it, and it runs where you run "
                    + "Keel."
                  : s.scope == "user" ? "You, in every project" : "This machine")
            actions {
                Button("Remove") { stream("/api/mcp/remove", ["name": s.name, "scope": s.scope]) }
                    .buttonStyle(QuietButton(tone: K.C.del))
                    .disabled(busy)
            }

        case .hook(let h):
            field("Event", h.event)
            // The command is the whole story for a hook, so it gets room and a monospace face.
            VStack(alignment: .leading, spacing: K.S.xs) {
                Text("COMMAND").font(.system(size: 10, weight: .semibold)).tracking(0.7)
                    .foregroundStyle(K.C.faint)
                Text(h.command)
                    .font(K.F.code).foregroundStyle(K.C.text)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(K.S.sm)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            }
            if !h.source.isEmpty { field("Declared in", h.source) }
            if h.fromRepo {
                warning("This is a shell command that came with the repository. It runs on your "
                        + "machine, around the agent's tool calls, without asking. Keel quarantines "
                        + "repository hooks before the agent starts — read it before restoring it.")
            }
            actions {
                Button("Copy command") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(h.command, forType: .string)
                }
                .buttonStyle(QuietButton())
            }

        case .plugin(let p):
            field("Marketplace", p.marketplace)
            field("State", p.enabled ? "Enabled" : "Disabled — installed but not loaded")
            actions {
                Button(p.enabled ? "Disable" : "Enable") {
                    stream("/api/plugins/action",
                           ["action": p.enabled ? "disable" : "enable", "name": p.name])
                }
                .buttonStyle(QuietButton()).disabled(busy)

                Button("Update") {
                    stream("/api/plugins/action", ["action": "update", "name": p.name])
                }
                .buttonStyle(QuietButton()).disabled(busy)

                Button("Uninstall") {
                    stream("/api/plugins/action", ["action": "uninstall", "name": p.name])
                }
                .buttonStyle(QuietButton(tone: K.C.del)).disabled(busy)
            }
        }
    }

    // MARK: Pieces

    private func field(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            Text(label.uppercased())
                .font(.system(size: 10, weight: .semibold)).tracking(0.7)
                .foregroundStyle(K.C.faint)
            Text(value)
                .font(K.F.body).foregroundStyle(K.C.text)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func pathField(_ path: String) -> some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            Text("FILE").font(.system(size: 10, weight: .semibold)).tracking(0.7)
                .foregroundStyle(K.C.faint)
            Text(path)
                .font(K.F.code).foregroundStyle(K.C.dim)
                .textSelection(.enabled)
                .lineLimit(2).truncationMode(.head)
        }
    }

    /// Skills live in `~/.claude`, outside the repository, so the daemon's repo-bounded reveal
    /// cannot open them. AppKit can, and it is the right tool for an absolute path anyway.
    private func revealButton(_ path: String) -> some View {
        Button("Reveal in Finder") {
            NSWorkspace.shared.selectFile(path, inFileViewerRootedAtPath: "")
        }
        .buttonStyle(QuietButton())
    }

    private func warning(_ text: String) -> some View {
        HStack(alignment: .top, spacing: K.S.sm) {
            Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 10))
            Text(text).fixedSize(horizontal: false, vertical: true)
        }
        .font(K.F.small).foregroundStyle(K.C.warn)
        .padding(K.S.sm)
        .background(K.C.warn.opacity(0.08), in: RoundedRectangle(cornerRadius: K.R.sm))
    }

    private func actions(@ViewBuilder _ content: () -> some View) -> some View {
        HStack(spacing: K.S.sm) { content(); Spacer() }
    }

    private func stream(_ path: String, _ query: [String: String]) {
        busy = true
        log = ""
        Task {
            defer { busy = false }
            do {
                for try await e in model.client.events(path, query) {
                    switch e.name {
                    case "line", "fatal": log += e.data + "\n"
                    case "done":
                        await model.refreshState()
                        model.inspecting = nil
                    default: break
                    }
                }
            } catch { log += error.localizedDescription }
        }
    }

    // MARK: Identity

    private var name: String {
        switch target {
        case .skill(let x), .agent(let x), .mcp(let x): x.name
        case .hook(let h): h.event
        case .plugin(let p): p.name
        }
    }

    private var icon: String {
        switch target {
        case .skill: "sparkles"
        case .agent: "person.2"
        case .mcp: "cable.connector"
        case .hook: "bolt.horizontal"
        case .plugin: "puzzlepiece.extension"
        }
    }

    private var fromRepo: Bool {
        switch target {
        case .skill(let x), .agent(let x), .mcp(let x): x.fromRepo
        case .hook(let h): h.fromRepo
        case .plugin: false
        }
    }
}
