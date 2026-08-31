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
        ScrollView {
            VStack(spacing: K.S.xl) {
                hero
                status
                remediation
                actions
                recents
                keys
            }
            // A column with a ceiling, centred. It was pinned to the top-left of whatever width
            // the window happened to be, so on a wide screen the whole product introduced itself
            // in the first quarter of the glass and left three quarters of nothing beside it.
            .frame(maxWidth: 720)
            .frame(maxWidth: .infinity, alignment: .center)
            .padding(.horizontal, K.S.xl)
            .padding(.vertical, K.S.xxl)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background {
            // One soft wash behind the title, in the accent. Nothing else on this screen is
            // coloured, so it reads as depth rather than as decoration — and it gives the page a
            // top, which a flat sheet of `bg` did not have.
            ZStack(alignment: .top) {
                K.C.bg
                RadialGradient(colors: [K.C.accent.opacity(0.10), .clear],
                               center: .top, startRadius: 0, endRadius: 520)
                    .frame(height: 520)
                    .allowsHitTesting(false)
            }
            .ignoresSafeArea()
        }
        .safeAreaInset(edge: .bottom) { footer }
        .task { await refresh() }
        .sheet(isPresented: $starting) {
            StartProject(client: model.client) { path, brief, file in
                starting = false
                open(path)
                if let file { model.attach(fileURL: file) }
                if !brief.isEmpty {
                    model.prompt = brief
                    Task {
                        try? await Task.sleep(for: .milliseconds(file == nil ? 800 : 1800))
                        model.send()
                    }
                }
            }
        }
    }

    private var hero: some View {
        VStack(spacing: K.S.md) {
            HStack(spacing: K.S.half) {
                Image(systemName: "sailboat.fill").font(K.F.small)
                Text("KEEL").font(K.F.micro.weight(.bold)).tracking(2.5)
            }
            .foregroundStyle(K.C.accent)

            // The visibility pillar, in the reader's own words. "Make a repository shippable" was
            // a claim about an outcome nobody has asked for yet, and "an easier, faster way to
            // work" is true of everything. "Actually" is the load-bearing word: it concedes that
            // today you cannot, which is the complaint this audience arrives with.
            Text("See what your agent actually did.")
                .font(K.F.hero).foregroundStyle(K.C.text)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: 640)
            Text("Keel drives the Claude Code already on this machine, against a repository "
                 + "already on this disk. Your subscription, your files. Nothing is uploaded "
                 + "and there is no account to make.")
                .font(K.F.reading).foregroundStyle(K.C.dim)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: 540)
        }
        .padding(.top, K.S.lg)
    }

    /// What Keel is about to drive, in one strip.
    ///
    /// Two label-and-value rows in a column, under a heading, for two facts that are each three
    /// words long. They are the answer to "will this work at all", so they belong beside each
    /// other and above the fold, not stacked in a section of their own.
    private var status: some View {
        HStack(spacing: 0) {
            statusItem(ok: claude?.installed == true, "Claude Code",
                       claude?.installed == true ? (claude?.version ?? "installed")
                                                 : "not found on your PATH")
            Rectangle().fill(K.C.line).frame(width: 1, height: 26)
            statusItem(ok: claude?.authenticated == true, "Signed in",
                       claude?.authenticated == true
                           ? [claude?.account, claude?.plan].compactMap { $0 }.joined(separator: " · ")
                           : "no account on this machine")
        }
        .padding(.vertical, K.S.sm)
        .background(K.C.surface, in: RoundedRectangle(cornerRadius: K.R.lg))
        .overlay(RoundedRectangle(cornerRadius: K.R.lg).stroke(K.C.line, lineWidth: 1))
    }

    private func statusItem(ok: Bool, _ label: String, _ detail: String) -> some View {
        HStack(spacing: K.S.sm) {
            Circle().fill(ok ? K.C.add : K.C.faint.opacity(0.4))
                .frame(width: 6, height: 6)
            VStack(alignment: .leading, spacing: K.S.hair) {
                Text(label).font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                Text(detail).font(K.F.micro).foregroundStyle(K.C.dim)
                    .lineLimit(1).truncationMode(.middle)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, K.S.lg)
    }

    /// The one thing standing between this machine and a working agent, when there is one.
    @ViewBuilder private var remediation: some View {
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
            .frame(maxHeight: 150)
            .padding(K.S.sm)
            .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
        }
        if let error {
            Text(error).font(K.F.small).foregroundStyle(K.C.del)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// Three ways in, weighted. Opening a folder is what nearly everybody is here to do, so it is
    /// the one that is filled; the other two are the same size and quieter rather than smaller,
    /// because a row of three different-sized buttons reads as one button and two mistakes.
    private var actions: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            HStack(spacing: K.S.sm) {
                Text("OPEN SOMETHING").sectionLabel().foregroundStyle(K.C.faint)
                if !ready {
                    Text("after the two above — the only things Keel cannot install for you")
                        .font(K.F.micro).foregroundStyle(K.C.faint)
                }
                Spacer(minLength: 0)
            }
            HStack(spacing: K.S.md) {
                ActionTile(icon: "folder.fill", title: "Open a folder",
                           detail: "A repository already on this disk", primary: true) {
                    openFolder()
                }
                ActionTile(icon: "wand.and.stars", title: "New project",
                           detail: "Scaffolded, with its own gate") { starting = true }
                ActionTile(icon: "arrow.down.circle.fill", title: "Clone from GitHub",
                           detail: "Bring one down and open it") { starting = true }
            }
            .disabled(!ready)
            .opacity(ready ? 1 : 0.45)
        }
    }

    @ViewBuilder private var recents: some View {
        if !Recents.paths.isEmpty {
            VStack(alignment: .leading, spacing: K.S.sm) {
                Text("RECENT").sectionLabel().foregroundStyle(K.C.faint)
                VStack(spacing: 0) {
                    // Six. The list was however many you had ever opened — eight of them on this
                    // machine — and a first screen whose longest element is a history is a screen
                    // about the past.
                    ForEach(Array(Recents.paths.prefix(6).enumerated()), id: \.element) { i, path in
                        if i > 0 { Rectangle().fill(K.C.line).frame(height: 1) }
                        HoverRow {
                            HStack(spacing: K.S.sm) {
                                Image(systemName: "folder").font(K.F.small)
                                    .foregroundStyle(K.C.accent)
                                Text((path as NSString).lastPathComponent)
                                    .font(K.F.body.weight(.medium)).foregroundStyle(K.C.text)
                                Text((path as NSString).deletingLastPathComponent)
                                    .font(K.F.codeTiny).foregroundStyle(K.C.faint)
                                    .lineLimit(1).truncationMode(.head)
                                Spacer(minLength: K.S.sm)
                                Image(systemName: "chevron.right").font(K.F.tiny)
                                    .foregroundStyle(K.C.faint)
                            }
                            .padding(.vertical, K.S.tight)
                        } action: {
                            if ready { open(path) }
                        }
                    }
                }
                .background(K.C.surface, in: RoundedRectangle(cornerRadius: K.R.lg))
                .overlay(RoundedRectangle(cornerRadius: K.R.lg).stroke(K.C.line, lineWidth: 1))
                .opacity(ready ? 1 : 0.45)
            }
        }
    }

    /// The three keys that make the window a keyboard tool, said once, here, before there is a
    /// menu bar item to find them in.
    private var keys: some View {
        HStack(spacing: K.S.md) {
            key("⌘K", "anything")
            key("⌘N", "another agent")
            key("⇧⇥", "Plan and Auto")
        }
        .foregroundStyle(K.C.faint)
    }

    private func key(_ chord: String, _ what: String) -> some View {
        HStack(spacing: K.S.half) {
            Text(chord).font(K.F.codeTiny)
                .padding(.horizontal, K.S.xs).padding(.vertical, K.S.hair)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
            Text(what).font(K.F.micro)
        }
    }

    private var footer: some View {
        Text("Keel sends crash reports and anonymous usage counts — never prompts, files "
             + "or repository names. Turn either off in Settings › Privacy.")
            .font(K.F.micro).foregroundStyle(K.C.faint)
            .multilineTextAlignment(.center)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity)
            .padding(K.S.md)
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


/// One way into the product: an icon, what it does, and what that means.
///
/// A tile rather than a button because this is the only screen where the choice *is* the content —
/// three bare buttons in a row said "Open a folder…", "New project…", "Clone from GitHub…" and
/// left the difference between the last two to the word "new".
private struct ActionTile: View {
    let icon: String
    let title: String
    let detail: String
    var primary = false
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: K.S.half) {
                Image(systemName: icon)
                    .font(K.F.title)
                    .foregroundStyle(primary ? K.C.accent : K.C.dim)
                    .padding(.bottom, K.S.sm)
                Text(title).font(K.F.body.weight(.semibold)).foregroundStyle(K.C.text)
                Text(detail).font(K.F.micro).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)
                    .multilineTextAlignment(.leading)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(K.S.md)
            .background(
                RoundedRectangle(cornerRadius: K.R.lg)
                    .fill(primary ? K.C.accent.wash : K.C.surface)
            )
            .overlay(
                RoundedRectangle(cornerRadius: K.R.lg)
                    .stroke(primary ? K.C.accent.opacity(hovering ? 0.7 : 0.4)
                                    : K.C.line, lineWidth: 1)
            )
            // The lift is the whole affordance: a card that does not answer the pointer reads as
            // a panel, and every panel on this screen is one you cannot click.
            .overlay(
                RoundedRectangle(cornerRadius: K.R.lg)
                    .fill(K.C.hover).opacity(hovering ? 1 : 0)
            )
            .contentShape(RoundedRectangle(cornerRadius: K.R.lg))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .hint("\(title) — \(detail)")
    }
}
