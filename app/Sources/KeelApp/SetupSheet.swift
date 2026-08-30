import SwiftUI

/// What this project needs before the agent can do its job here, as a checklist with the fix
/// on each line.
///
/// Shown once per project on open or clone when anything is missing — a tool, a sign-in, a
/// recommended plugin, a gate, git — and again from ⌘K "Project setup". It was nine badges on
/// nine icons, which told the person there were problems and nothing about which to fix first.
struct SetupSheet: View {
    @Bindable var model: SessionModel
    let done: () -> Void
    @State private var openSettings = false

    struct Item: Identifiable {
        let id: String
        let icon: String
        let title: String
        let detail: String
        let action: String
        let tone: Pill.Tone
        /// Work in flight for this item — the row shows it, and stays until the item clears.
        var busy = false
        let run: () -> Void
    }

    static func items(for m: SessionModel, settings: @escaping () -> Void) -> [Item] {
        var out: [Item] = []
        if !m.isRepo {
            out.append(Item(id: "git", icon: "arrow.triangle.branch", title: "Not a git repository",
                            detail: "Commits, rewind and pull requests need one.",
                            action: "git init", tone: .bad) { Task { _ = await m.gitInit() } })
        }
        if m.gateCommand == nil {
            // Sent, not drafted: the click is the ask. The sheet stays; the row shows the
            // agent working and clears itself when a gate exists.
            out.append(Item(id: "gate", icon: "checkmark.seal", title: "No gate",
                            detail: "Nothing checks the agent's work. Keel runs the project's own tests after every turn — it needs a `check` target, a test script, or `cargo test`.",
                            action: "Add one", tone: .warn, busy: m.running) {
                guard !m.running else { return }
                m.mode = "acceptEdits"
                m.prompt = "Add a gate to this project: a `check` target in the Makefile (or a `check` script in package.json) that runs the existing lint, typecheck and tests. Use only commands the repository already has. Run it once to prove it passes."
                m.send()
            })
        }
        for t in m.tools where !t.installed || !t.authenticated {
            out.append(Item(id: "tool-" + t.id, icon: "wrench.and.screwdriver",
                            title: t.installed ? "\(t.label) is not signed in" : "\(t.label) is not installed",
                            detail: t.blocked ?? (t.installed ? "Sign in so deploys and clones work." : "Install it from Settings › Tools."),
                            action: "Open Settings › Tools", tone: t.installed ? .warn : .neutral) { settings() })
        }
        for e in m.missingSuggestions {
            out.append(Item(id: "plugin-" + e.id, icon: "puzzlepiece.extension",
                            title: "Recommended: \(e.name)",
                            detail: e.reason ?? e.description ?? "",
                            action: "Install", tone: .accent, busy: m.installing == e.id) { Task { await m.installPlugin(e) } })
        }
        let repoHooks = m.workspace.hooks.filter(\.fromRepo).count
        if repoHooks > 0 {
            out.append(Item(id: "hooks", icon: "bolt.horizontal",
                            title: "\(repoHooks) hook\(repoHooks == 1 ? "" : "s") came with the repository",
                            detail: "A hook is a shell command that runs on this machine. Keel quarantined them; review before enabling.",
                            action: "Review in Hooks", tone: .bad) { m.inspecting = nil; m.sheet = nil; NotificationCenter.default.post(name: .keelTogglePanel, object: nil) })
        }
        if !m.trusted {
            out.append(Item(id: "trust", icon: "checkmark.shield",
                            title: "Commands are approved one at a time",
                            detail: "On a repository you own, trusting it removes the per-command question. Withdrawable from the status bar.",
                            action: "Trust this project…", tone: .neutral) { NotificationCenter.default.post(name: .keelTrust, object: nil) })
        }
        return out
    }

    private static func done_(_ m: SessionModel) { m.sheet = nil; m.focusComposerTick += 1 }

    private var items: [Item] {
        Self.items(for: model) { openSettings = true; done(); NotificationCenter.default.post(name: .keelSettings, object: nil) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            HStack {
                VStack(alignment: .leading, spacing: K.S.xxs) {
                    Text("Project setup").font(K.F.title).foregroundStyle(K.C.text)
                    Text((model.repoPath as NSString).lastPathComponent)
                        .font(K.F.codeSmall).foregroundStyle(K.C.faint)
                }
                Spacer()
                CloseButton(size: 10, label: "Close") { done() }
            }

            if items.isEmpty {
                HStack(spacing: K.S.sm) {
                    Image(systemName: "checkmark.circle.fill").foregroundStyle(K.C.add)
                    Text("Everything is in place. Ask for a change.").font(K.F.body).foregroundStyle(K.C.dim)
                }
                .padding(.vertical, K.S.md)
            } else {
                Text("\(items.count) thing\(items.count == 1 ? "" : "s") to sort out. None blocks a first turn; "
                     + "each makes the agent more useful here.")
                    .font(K.F.small).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)
                VStack(spacing: 0) {
                    ForEach(items) { it in
                        HStack(alignment: .top, spacing: K.S.sm) {
                            Image(systemName: it.icon).font(K.F.small)
                                .foregroundStyle(K.C.dim).frame(width: 18).padding(.top, K.S.xxs)
                            VStack(alignment: .leading, spacing: K.S.xxs) {
                                Text(it.title).font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                                Text(it.detail).font(K.F.micro).foregroundStyle(K.C.dim)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                            Spacer(minLength: K.S.sm)
                            if it.busy {
                                HStack(spacing: K.S.xs) {
                                    ProgressView().controlSize(.mini)
                                    Text("working…").font(K.F.micro).foregroundStyle(K.C.faint)
                                }
                            } else {
                                Button(it.action) { it.run() }
                                    .buttonStyle(QuietButton(tone: it.tone == .accent || it.tone == .warn ? K.C.accent : K.C.dim))
                            }
                        }
                        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
                        if it.id != items.last?.id { Hairline() }
                    }
                }
                .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.md))
                .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))
            }

            HStack {
                Text("⌘K › Project setup brings this back.").font(K.F.micro).foregroundStyle(K.C.faint)
                Spacer()
                Button("Done") { done() }.buttonStyle(FilledButton())
            }
        }
        .padding(K.S.xl)
        .frame(width: 560)
        .background(K.C.bg)
    }
}
