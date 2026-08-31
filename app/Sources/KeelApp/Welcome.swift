import AppKit
import SwiftUI

/// The first screen, and the one every launch lands on.
///
/// Keel does not reopen the project you closed last. With several projects, lanes and worktrees in
/// play, being dropped somewhere and having to read the title bar to find out where is worse than
/// being asked — and RECENT below makes the answer one click.
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
    /// Why the daemon would not start, when it would not. Read from `AppModel`, which had it all
    /// along and rendered it only in the torn-out window.
    var daemonFailure: String? = nil
    let onOpened: () -> Void

    @State private var claude: ClaudeStatus?
    /// Why the status probe failed, when it did. Not the same as "claude is missing": if Keel's
    /// own service is not answering, installing the CLI fixes nothing.
    @State private var unreachable: String?
    @State private var installing = false
    @State private var log = ""
    @State private var error: String?

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
        VStack(alignment: .leading, spacing: K.S.xl) {
            VStack(alignment: .leading, spacing: K.S.sm) {
                Text("KEEL").font(K.F.micro.weight(.bold)).tracking(2.5)
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

            Text("Keel sends crash reports and anonymous usage counts — never prompts, files "
                 + "or repository names. Turn either off in Settings › Privacy.")
                .font(K.F.micro).foregroundStyle(K.C.faint)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(K.S.xxl)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(K.C.bg)
        .task { await refresh() }
    }

    private var claudeSection: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            Text("CLAUDE CODE").sectionLabel()
                .foregroundStyle(K.C.faint)

            row(ok: claude?.installed == true, "Installed",
                claude?.installed == true ? (claude?.version ?? "") : "not found on your PATH")
            row(ok: claude?.authenticated == true, "Signed in",
                claude?.authenticated == true
                    ? [claude?.account, claude?.plan].compactMap { $0 }.joined(separator: " · ")
                    : "no account on this machine")

            if let why = daemonFailure ?? unreachable {
                fix("Keel's local service is not answering, so it cannot see anything about your "
                    + "machine yet. \(why)") {
                    Button("Try again") { Task { await refresh() } }
                        .buttonStyle(FilledButton())
                }
            } else if claude?.installed != true {
                fix("Keel drives your own `claude` and has nothing to run without it. It needs no "
                    + "Node and no Homebrew.") {
                    Button(installing ? "Installing…" : "Install Claude Code") {
                        Task { await install() }
                    }
                    .buttonStyle(FilledButton())
                    .disabled(installing)
                }
            } else if claude?.authenticated != true {
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
        VStack(alignment: .leading, spacing: K.S.sm) {
            HStack(spacing: K.S.sm) {
                Text("OPEN SOMETHING").sectionLabel()
                    .foregroundStyle(K.C.faint)
                if !ready {
                    Text("after the two above — the only things Keel cannot install for you")
                        .font(K.F.micro).foregroundStyle(K.C.faint)
                }
            }

            HStack(spacing: K.S.sm) {
                Button("Open a folder…") { openFolder() }.buttonStyle(FilledButton())
            }
            .disabled(!ready)
            .opacity(ready ? 1 : 0.45)

            // The three keys that make the window a keyboard tool, said once, here, before
            // there is a menu bar item to find them in.
            Text("⌘K for anything · ⌘N for another agent · ⇧⇥ to switch Plan and Auto")
                .font(K.F.micro).foregroundStyle(K.C.faint)

            if let error {
                Text(error).font(K.F.small).foregroundStyle(K.C.del)
            }

            if !Recents.paths.isEmpty {
                Text("RECENT").sectionLabel()
                    .foregroundStyle(K.C.faint).padding(.top, K.S.sm)
                ForEach(Recents.paths, id: \.self) { path in
                    HoverRow {
                        HStack(spacing: K.S.sm) {
                            Image(systemName: "folder").font(K.F.tiny)
                                .foregroundStyle(K.C.faint)
                            Text((path as NSString).lastPathComponent)
                                .font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                            Text((path as NSString).deletingLastPathComponent)
                                .font(K.F.codeTiny).foregroundStyle(K.C.faint)
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
        do {
            claude = try await model.client.get("/api/claude")
            unreachable = nil
        } catch {
            // This used to be `try?`, so a daemon that never started left `claude` nil — and both
            // remediations below tested `== false`, which nil is not. The first-run screen showed
            // two grey dots, three disabled buttons and no explanation at all.
            claude = nil
            unreachable = error.localizedDescription
        }
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

