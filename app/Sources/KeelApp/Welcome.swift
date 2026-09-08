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
    var onSettings: () -> Void
    let onOpened: () -> Void

    @State private var setup: ClaudeSetup
    private var claude: ClaudeSetup.Status? { setup.status }
    /// Why the status probe failed, when it did. Not the same as "claude is missing": if Keel's
    /// own service is not answering, installing the CLI fixes nothing.
    @State private var unreachable: String?
    @State private var error: String?
    @State private var starting = false
    /// Which form `StartProject` opens on, set by the tile that opened it.
    @State private var startMode = StartProject.Mode.new
    /// A git repository with no `CLAUDE.md`, waiting on the answer to the dialog below.
    @State private var uninitialised: String?

    init(model: SessionModel, daemonFailure: String? = nil,
         onSettings: @escaping () -> Void = {}, onOpened: @escaping () -> Void) {
        self.model = model
        self.daemonFailure = daemonFailure
        self.onSettings = onSettings
        self.onOpened = onOpened
        _setup = State(initialValue: ClaudeSetup(client: model.client))
    }

    private var ready: Bool { setup.ready }

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
        .onDisappear { setup.cancel() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            if !setup.busy { Task { await refresh() } }
        }
        .confirmationDialog("Set Claude up in this project?",
                            isPresented: .init(get: { uninitialised != nil },
                                               set: { if !$0 { uninitialised = nil } }),
                            titleVisibility: .visible) {
            Button("Set it up") {
                guard let path = uninitialised else { return }
                uninitialised = nil
                finishOpening(path, initialise: true)
            }
            .keyboardShortcut(.defaultAction)
            Button("Open without it") {
                guard let path = uninitialised else { return }
                uninitialised = nil
                finishOpening(path, initialise: false)
            }
            Button("Cancel", role: .cancel) { uninitialised = nil }
        } message: {
            Text("This repository has no CLAUDE.md, so the agent starts with nothing about the "
                 + "project — how to build it, what the gate is, which files matter. Recommended: "
                 + "Keel opens it and runs `/init`, which reads the repository and writes one. It "
                 + "is a normal turn, so you see it and can rewind it.")
        }
        .sheet(isPresented: $starting) {
            StartProject(client: model.client, start: startMode) { path, brief, file in
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
            Text("Your Claude account. A native workspace for everything it builds. "
                 + "Install, sign in, and open a project—Keel guides you through it.")
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
        } else if !ready || setup.failure != nil {
            ClaudeSetupCard(setup: setup, showsStatus: false)
        }
        if let error {
            Text(error).font(K.F.small).foregroundStyle(K.C.del)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// Two ways in, weighted (three with scaffolding flagged on). Opening a folder is what nearly
    /// everybody is here to do, so it is
    /// the one that is filled; the other two are the same size and quieter rather than smaller,
    /// because a row of three different-sized buttons reads as one button and two mistakes.
    private var actions: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            HStack(spacing: K.S.sm) {
                Text("OPEN SOMETHING").sectionLabel().foregroundStyle(K.C.faint)
                if !ready {
                    Text("available after Claude Code is connected")
                        .font(K.F.micro).foregroundStyle(K.C.faint)
                }
                Spacer(minLength: 0)
            }
            HStack(spacing: K.S.md) {
                ActionTile(icon: "folder.fill", title: "Open a folder",
                           detail: "A repository already on this disk", primary: true) {
                    openFolder()
                }
                if Flags.scaffolding {
                    ActionTile(icon: "wand.and.stars", title: "New project",
                               detail: "Scaffolded, with its own gate") {
                        startMode = .new
                        starting = true
                    }
                }
                ActionTile(icon: "arrow.down.circle.fill", title: "Clone from GitHub",
                           detail: "Bring one down and open it") {
                    startMode = .clone
                    starting = true
                }
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
            Button(action: onSettings) {
                Label("Tools & settings", systemImage: "gearshape")
            }
            .buttonStyle(QuietButton())
            .help("Install tools and manage accounts before opening a project (⌘,)")
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
        Text("Crash reports and usage events are enabled when configured. "
             + "Turn either off in Settings › Privacy.")
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
        await setup.refresh()
        unreachable = setup.status == nil ? setup.failure : nil
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

    /// Open a folder, refusing one git does not manage and offering to initialise Claude in one it
    /// has never been run in.
    ///
    /// The git check is repeated here so the refusal names the folder the person just picked
    /// instead of arriving as an HTTP error; `open_repo` is the one that holds, because recents and
    /// the project menu open by other routes.
    ///
    /// The `/init` is asked for rather than assumed. It is a turn — it costs tokens, it writes a
    /// file into their repository, and a person who keeps their instructions in `AGENTS.md` or a
    /// `.claude` directory has not made a mistake. Asked before the open, not after, because the
    /// welcome screen is gone the moment the project is open and a dialog on a dismissed view is a
    /// dialog nobody sees.
    private func open(_ path: String) {
        let git = (path as NSString).appendingPathComponent(".git")
        guard FileManager.default.fileExists(atPath: git) else {
            error = "\((path as NSString).lastPathComponent) is not a git repository. Keel works "
                  + "with git-managed folders only — run `git init` in it first, or open one that "
                  + "is already tracked."
            return
        }
        let claudeMd = (path as NSString).appendingPathComponent("CLAUDE.md")
        if !FileManager.default.fileExists(atPath: claudeMd) {
            uninitialised = path
            return
        }
        finishOpening(path, initialise: false)
    }

    private func finishOpening(_ path: String, initialise: Bool) {
        Task {
            do {
                try await model.openProject(path)
                Recents.remember(path)
                onOpened()
                if initialise {
                    // Claude Code's own `/init`, sent verbatim — it resolves the command itself,
                    // and this is the first thing the person would type anyway.
                    model.prompt = "/init"
                    try? await Task.sleep(for: .milliseconds(800))
                    model.send()
                }
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
