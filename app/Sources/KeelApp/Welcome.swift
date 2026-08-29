import AppKit
import SwiftUI

/// The first screen, and the one a launch lands on when the remembered project has moved.
///
/// It asks for one thing — a project — and reports one thing: whether the `claude` it is about to
/// drive is actually usable. Opening a project before that works gets you a chat box that cannot
/// answer, which is a dead end offered as a feature.
///
/// Homebrew is deliberately *not* a gate. It exists to install `gh` and friends, none of which the
/// agent loop needs, and the web UI blocked "open a folder" on it three lines above a footer
/// saying those tools were not needed to start. Both could not be true.
struct Welcome: View {
    let model: SessionModel
    let onOpened: () -> Void

    @State private var claude: ClaudeStatus?
    @State private var installing = false
    @State private var log = ""
    @State private var error: String?
    @State private var starting = false

    struct ClaudeStatus: Decodable {
        var installed: Bool
        var version: String?
        var authenticated: Bool
        var account: String?
        var plan: String?
        var brew: Bool
    }

    private var ready: Bool { claude?.installed == true && claude?.authenticated == true }

    var body: some View {
        VStack(alignment: .leading, spacing: 22) {
            VStack(alignment: .leading, spacing: K.S.sm) {
                Text("KEEL").font(.system(size: 11, weight: .bold)).tracking(2.5)
                    .foregroundStyle(K.C.faint)
                Text("Make a repository shippable.")
                    .font(K.F.display).foregroundStyle(K.C.text)
                Text("Keel drives the Claude Code already on this machine, against a repository "
                     + "already on this disk. Your subscription, your files. Nothing is uploaded "
                     + "and there is no account to make.")
                    .font(K.F.body).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: 520, alignment: .leading)
            }

            claudeSection
            openSection

            Spacer()
        }
        .padding(K.S.xxl)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(K.C.bg)
        .task { await refresh() }
        .sheet(isPresented: $starting) {
            StartProject(client: model.client) { path in
                starting = false
                open(path)
            }
        }
    }

    private var claudeSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("CLAUDE CODE").font(.system(size: 9.5, weight: .semibold)).tracking(0.7)
                .foregroundStyle(K.C.faint)

            row(ok: claude?.installed == true, "Installed",
                claude?.installed == true ? (claude?.version ?? "") : "not found on your PATH")
            row(ok: claude?.authenticated == true, "Signed in",
                claude?.authenticated == true
                    ? [claude?.account, claude?.plan].compactMap { $0 }.joined(separator: " · ")
                    : "no account on this machine")

            if claude?.installed == false {
                fix("Keel drives your own `claude` and has nothing to run without it. It needs no "
                    + "Node and no Homebrew.") {
                    Button(installing ? "Installing…" : "Install Claude Code") {
                        Task { await install() }
                    }
                    .buttonStyle(SendButtonWide())
                    .disabled(installing)
                }
            } else if claude?.authenticated == false {
                // The sign-in is a browser handshake the CLI drives itself. Keel cannot do it for
                // you, and pretending otherwise would fail later inside a chat.
                fix("Run `claude` once and sign in — it opens a browser, which Keel cannot drive "
                    + "for you. Use the terminal in any session window.") { EmptyView() }
            }

            if !log.isEmpty {
                ScrollView {
                    Text(log).font(K.F.codeSmall).foregroundStyle(K.C.dim)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxWidth: 520, maxHeight: 150)
                .padding(K.S.sm)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            }
        }
    }

    private var openSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: K.S.sm) {
                Text("OPEN SOMETHING").font(.system(size: 9.5, weight: .semibold)).tracking(0.7)
                    .foregroundStyle(K.C.faint)
                if !ready {
                    Text("after the two above — the only things Keel cannot install for you")
                        .font(K.F.micro).foregroundStyle(K.C.faint)
                }
            }

            HStack(spacing: K.S.sm) {
                Button("Open a folder…") { openFolder() }.buttonStyle(SendButtonWide())
                Button("New project…") { starting = true }.buttonStyle(QuietButton())
                Button("Clone from GitHub…") { starting = true }.buttonStyle(QuietButton())
            }
            .disabled(!ready)
            .opacity(ready ? 1 : 0.45)

            if let error {
                Text(error).font(K.F.small).foregroundStyle(K.C.del)
            }

            if !Recents.paths.isEmpty {
                Text("RECENT").font(.system(size: 9, weight: .semibold)).tracking(0.7)
                    .foregroundStyle(K.C.faint).padding(.top, K.S.sm)
                ForEach(Recents.paths, id: \.self) { path in
                    HoverRow {
                        HStack(spacing: K.S.sm) {
                            Image(systemName: "folder").font(.system(size: 9))
                                .foregroundStyle(K.C.faint)
                            Text((path as NSString).lastPathComponent)
                                .font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                            Text((path as NSString).deletingLastPathComponent)
                                .font(K.F.mono(10)).foregroundStyle(K.C.faint)
                                .lineLimit(1).truncationMode(.head)
                        }
                    } action: {
                        if ready { open(path) }
                    }
                    .frame(maxWidth: 520, alignment: .leading)
                    .opacity(ready ? 1 : 0.45)
                }
            }
        }
    }

    private func row(ok: Bool, _ label: String, _ detail: String) -> some View {
        HStack(spacing: K.S.sm) {
            Circle().fill(ok ? K.C.add : K.C.faint.opacity(0.4))
                .frame(width: 6, height: 6)
            Text(label).font(K.F.body.weight(.medium)).foregroundStyle(K.C.text)
                .frame(width: 80, alignment: .leading)
            Text(detail).font(K.F.small).foregroundStyle(K.C.dim)
        }
    }

    private func fix(_ text: String, @ViewBuilder action: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            Text(.init(text)).font(K.F.small).foregroundStyle(K.C.dim)
                .fixedSize(horizontal: false, vertical: true)
            action()
        }
        .padding(K.S.md)
        .frame(maxWidth: 520, alignment: .leading)
        .background(K.C.surface, in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))
    }

    private func refresh() async {
        claude = try? await model.client.get("/api/claude")
    }

    /// The vendor's own installer, streamed. It needs no npm and no Node and refuses to run under
    /// sudo, which is why it can be shown here rather than handed to a terminal.
    private func install() async {
        installing = true
        log = ""
        defer { installing = false }
        do {
            for try await event in model.client.events("/api/claude/install") {
                switch event.name {
                case "line", "fatal": log += event.data + "\n"
                case "done":
                    // Re-read rather than assume: an installer can succeed and still leave the
                    // binary somewhere this process cannot see.
                    await refresh()
                    if claude?.installed != true {
                        log += "\nInstalled, but Keel still cannot see it. Quit and reopen Keel — "
                             + "it reads your shell's PATH at startup.\n"
                    }
                default: break
                }
            }
        } catch {
            log += error.localizedDescription
        }
    }

    private func openFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Open"
        panel.message = "Choose a repository"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        open(url.path)
    }

    private func open(_ path: String) {
        Task {
            do {
                try await model.openProject(path)
                Recents.remember(path)
                onOpened()
            } catch {
                self.error = error.localizedDescription
            }
        }
    }
}

/// Projects opened before, newest first.
///
/// In `UserDefaults` rather than in the daemon: it is a convenience for this machine's UI, not
/// state the CLI needs to agree with.
enum Recents {
    private static let key = "keel.recents"

    static var paths: [String] {
        (UserDefaults.standard.array(forKey: key) as? [String] ?? [])
            .filter { FileManager.default.fileExists(atPath: $0) }
    }

    static func remember(_ path: String) {
        var list = paths.filter { $0 != path }
        list.insert(path, at: 0)
        UserDefaults.standard.set(Array(list.prefix(8)), forKey: key)
    }
}


/// The one prominent action on a screen. Same shape as Send, wider padding.
struct SendButtonWide: ButtonStyle {
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(K.F.small.weight(.medium))
            .foregroundStyle(enabled ? Color.white : K.C.faint)
            .padding(.horizontal, K.S.md).padding(.vertical, 5)
            .background(
                RoundedRectangle(cornerRadius: K.R.sm)
                    .fill(enabled ? K.C.accent.opacity(configuration.isPressed ? 0.75 : 1)
                                  : K.C.line)
            )
    }
}
