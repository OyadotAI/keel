import AppKit
import SwiftUI

/// One window, one conversation.
///
/// An activity rail, a panel, the stage, the conversation, and a status bar — laid out by hand
/// rather than by `NavigationSplitView`, which insists on sidebar chrome, its own toolbar
/// behaviour and a translucency that fights a dense surface.
struct SessionWindow: View {
    @State var lanes: Lanes
    let pairing: PairingModel
    @State private var panel: Panel? = .changes
    @State private var stage: Stage = .turn
    @State private var showTerminal = false
    @State private var terminalTitle = "shell"
    @State private var paletteOpen = false
    @State private var starting = false
    @State private var showSettings = false
    /// How wide the record beside the conversation is. Yours to drag; remembered.
    @AppStorage("keel.stageWidth") private var stageWidth: Double = 460

    /// The two things the right pane can be.
    ///
    /// "Trace" rather than "Turn": the pane is the record of what the agent did, and a turn is the
    /// unit inside it. "Designer" rather than "Preview": you do not only look at the page there,
    /// you pick things in it and change them.
    enum Stage: String, CaseIterable { case turn = "Trace", preview = "Designer" }

    /// One icon per thing, because they are different things. Grouping skills, subagents, MCP
    /// servers, hooks and plugins into one "Workspace" panel meant five headings fighting for a
    /// 256pt rail and no room for any of them to have actions.
    enum Panel: String, CaseIterable, Identifiable {
        case changes, files, sessions, readiness
        case skills, agents, mcp, hooks, plugins
        var id: String { rawValue }

        var icon: String {
            switch self {
            case .changes: "plusminus"
            case .files: "folder"
            case .sessions: "clock.arrow.circlepath"
            case .readiness: "checkmark.shield"
            case .skills: "sparkles"
            case .agents: "person.2"
            case .mcp: "cable.connector"
            case .hooks: "bolt.horizontal"
            case .plugins: "puzzlepiece.extension"
            }
        }
        var title: String {
            switch self {
            case .changes: "Changes"
            case .files: "Files"
            case .sessions: "History"
            case .readiness: "Readiness"
            case .skills: "Skills"
            case .agents: "Subagents"
            case .mcp: "MCP servers"
            case .hooks: "Hooks"
            case .plugins: "Plugins"
            }
        }

        /// A separator after these, so the repository group and the agent-configuration group read
        /// as two sets rather than nine icons in a column.
        var endsGroup: Bool { self == .readiness }
    }

    /// The lane in focus. Every pane below draws this one; the rail shows all of them.
    private var model: SessionModel { lanes.active ?? lanes.lanes[0] }

    var body: some View {
        Group {
            if model.projectOpen { workbench } else { Welcome(model: model) { } }
        }
        .background(K.C.bg)
        .task {
            await lanes.refreshShared()
            // Restore once the project is known: the saved lanes are keyed by it.
            await lanes.restore(repo: model.repoPath)
        }
        // Anything that changes which conversations are open is worth writing down: a lane that
        // has just been given a session id, one closed, or a different one focused.
        .onChange(of: lanes.lanes.compactMap(\.sessionId)) { lanes.remember(repo: model.repoPath) }
        .onChange(of: lanes.activeID) { lanes.remember(repo: model.repoPath) }
    }

    private var workbench: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                ActivityRail(panel: $panel, model: model) {
                    withAnimation(K.M.quick) { showSettings.toggle() }
                }

                // The lanes are always on screen. They are the reason this is a window and not a
                // terminal: several agents working at once, and you can see all of them.
                VStack(spacing: 0) {
                    LaneRail(lanes: lanes)
                    Hairline()
                    if let panel {
                        SidePanel(panel: panel, model: model)
                    } else {
                        Spacer()
                    }
                }
                // Collapsing the panel gives the width back; the lanes alone need less.
                .frame(width: panel == nil ? 200 : 256)
                Rectangle().fill(K.C.line).frame(width: 1)

                if showSettings {
                    SettingsPage(model: model, pairing: pairing) {
                        withAnimation(K.M.quick) { showSettings = false }
                    }
                    .frame(maxWidth: .infinity)
                } else {
                    working
                }
            }
            Hairline()
            StatusBar(model: model, terminalOpen: $showTerminal)
        }
        .overlay(alignment: .top) { paletteOverlay }
        .toolbar { toolbar }
        .navigationTitle(model.title)
        .navigationSubtitle(subtitle)
        .modifier(WindowEvents(
            lanes: lanes, model: model,
            stage: $stage, showSettings: $showSettings, showTerminal: $showTerminal,
            paletteOpen: $paletteOpen, starting: $starting, panel: $panel))
        .sheet(isPresented: $starting) {
            StartProject(client: model.client) { path in
                starting = false
                Task {
                    try? await model.openProject(path)
                    Recents.remember(path)
                    await lanes.refreshShared()
                }
            }
        }
    }

    @ViewBuilder
    private var paletteOverlay: some View {
        if paletteOpen {
            Palette(model: model, open: $paletteOpen)
                .padding(.top, 60)
                .transition(.scale(scale: 0.98).combined(with: .opacity))
        }
    }

    /// The working area: conversation in the middle, the record beside it.
    private var working: some View {
        HStack(spacing: 0) {
            // The conversation is the middle, because it is what you are doing. The record of what
            // the agent changed is a reference you consult, so it sits beside it.
            VStack(spacing: 0) {
                ChatRail(model: model)
                if showTerminal {
                    Hairline()
                    terminalPane
                }
            }
            .frame(minWidth: 420)

            SplitHandle(width: $stageWidth, range: 340...720, reset: 460)

            VStack(spacing: 0) {
                stageBar
                Hairline()
                if model.running { WorkingBar(model: model) }
                Group {
                    if let target = model.inspecting {
                        Inspector(model: model, target: target)
                    } else if let path = model.viewingDiff {
                        DiffSurface(model: model, path: path)
                    } else {
                        switch stage {
                        case .turn: TurnStage(model: model)
                        case .preview: PreviewSurface(model: model)
                        }
                    }
                }
            }
            .frame(width: stageWidth)
        }
    }

    private var subtitle: String {
        let branch = model.branch ?? ""
        return model.trusted ? (branch.isEmpty ? "trusted" : branch + " · trusted") : branch
    }

    /// A segmented control drawn by hand: the stock one is a rounded capsule that reads as iOS.
    private var stageBar: some View {
        HStack(spacing: K.S.xs) {
            ForEach(Stage.allCases, id: \.self) { s in
                let on = stage == s && model.viewingDiff == nil && model.inspecting == nil
                Text(s.rawValue)
                    .font(K.F.small.weight(on ? .semibold : .regular))
                    .foregroundStyle(on ? K.C.text : K.C.faint)
                    .padding(.horizontal, K.S.sm).padding(.vertical, 3)
                    .background(
                        RoundedRectangle(cornerRadius: K.R.sm)
                            .fill(on ? K.C.text.opacity(0.07) : .clear)
                    )
                    .contentShape(Rectangle())
                    .asButton { stage = s; model.viewingDiff = nil; model.inspecting = nil }
                    .accessibilityAddTraits(on ? .isSelected : [])
            }
            Spacer()
        }
        .padding(.horizontal, K.S.md)
        .padding(.vertical, K.S.sm)
        .background(K.C.surface)
    }

    private var terminalPane: some View {
        VStack(spacing: 0) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "terminal").font(.system(size: 10)).foregroundStyle(K.C.faint)
                Text(terminalTitle).font(K.F.mono(10)).foregroundStyle(K.C.dim)
                Spacer()
                CloseButton(size: 10) { withAnimation(K.M.quick) { showTerminal = false } }
            }
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.xs)
            .background(K.C.surface)
            TerminalPane(port: model.port, worktree: model.worktree, title: $terminalTitle)
                .id(model.id)
        }
        .frame(minHeight: 140, idealHeight: 220)
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        // The project, top left, where every editor puts it. It was a small label in the status
        // bar, and nobody found out it was a menu until they were told.
        ToolbarItem(placement: .navigation) {
            ProjectMenu(model: model, prominent: true)
        }
        ToolbarItem(placement: .primaryAction) {
            Button { withAnimation(K.M.quick) { paletteOpen.toggle() } } label: {
                Image(systemName: "command")
            }
            .hint("Command palette (⌘K)")
        }
        ToolbarItem(placement: .primaryAction) {
            Button { withAnimation(K.M.quick) { showTerminal.toggle() } } label: {
                Image(systemName: "terminal")
            }
            .hint("Terminal (⌘⌥T)")
        }
    }
}

// MARK: - Activity rail

/// Icon-only, 44pt, with a selection bar. The panel it opens is the one you were last in, and
/// clicking the active icon collapses the panel — the two gestures every editor has.
struct ActivityRail: View {
    @Binding var panel: SessionWindow.Panel?
    let model: SessionModel
    var onSettings: () -> Void = {}

    var body: some View {
        VStack(alignment: .center, spacing: K.S.xxs) {
            ForEach(SessionWindow.Panel.allCases) { p in
                RailButton(
                    icon: p.icon,
                    help: help(p),
                    badge: badge(p),
                    badgeTone: p == .hooks || p == .readiness ? K.C.warn : K.C.accent,
                    selected: panel == p
                ) {
                    withAnimation(K.M.quick) { panel = (panel == p) ? nil : p }
                }
                if p.endsGroup {
                    Rectangle().fill(K.C.line)
                        .frame(width: 18, height: 1)
                        .padding(.vertical, K.S.xs)
                }
            }
            Spacer()
            // `SettingsLink`, not a selector by name. `NSApp.sendAction(Selector("showSettingsWindow:"))`
            // fails silently when the selector does not match the OS version, which is exactly
            // what it was doing — the button was wired to nothing.
            Button { onSettings() } label: {
                Image(systemName: "gearshape")
                    .font(.system(size: 14))
                    .foregroundStyle(toolsNeedAttention > 0 ? K.C.warn : K.C.faint)
                    .frame(width: 44, height: 32)
                    .overlay(alignment: .topTrailing) {
                        if toolsNeedAttention > 0 {
                            Text("\(toolsNeedAttention)")
                                .font(.system(size: 10, weight: .bold))
                                .foregroundStyle(K.C.bg)
                                .padding(.horizontal, 3).padding(.vertical, 1)
                                .background(K.C.warn, in: Capsule())
                                .offset(x: -4, y: 2)
                        }
                    }
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .hint(toolsNeedAttention > 0
                  ? "Settings (⌘,) — \(toolsNeedAttention) tool\(toolsNeedAttention == 1 ? "" : "s") "
                    + "not installed or not signed in"
                  : "Settings (⌘,)")
        }
        .padding(.vertical, K.S.sm)
        .frame(width: 44)
        .background(K.C.surface)
        .overlay(alignment: .trailing) { Rectangle().fill(K.C.line).frame(width: 1) }
    }

    /// The tooltip carries the reason for the badge, so a number on an icon is not a riddle.
    private func help(_ p: SessionWindow.Panel) -> String {
        let n = model.missingSuggestions.count
        switch p {
        case .skills, .plugins where n > 0:
            return "\(p.title) — \(n) suggested for this repository, not installed"
        case .hooks:
            let repo = model.workspace.hooks.filter(\.fromRepo).count
            return repo > 0
                ? "Hooks — \(repo) came with this repository"
                : p.title
        default:
            return p.title
        }
    }

    /// CLIs that are missing or not signed in.
    ///
    /// Surfaced on the gear because that is where the fix is, and because a tool you only discover
    /// is missing when a deploy fails is a tool you discover at the worst moment.
    private var toolsNeedAttention: Int {
        model.tools.count { !$0.installed || !$0.authenticated }
    }

    private func badge(_ p: SessionWindow.Panel) -> Int? {
        switch p {
        case .changes: model.changes.count.nonZero
        case .readiness:
            model.findings.filter { $0.severity == "critical" || $0.severity == "high" }
                .count.nonZero
        // Hooks that came with the repository are the one count worth shouting: each is a shell
        // command someone else wrote that runs on this machine.
        case .hooks: model.workspace.hooks.filter(\.fromRepo).count.nonZero
        // What this repository is missing. A recommendation nobody sees does nothing, and both
        // panels can install it, so both carry the count.
        case .skills, .plugins: model.missingSuggestions.count.nonZero
        default: nil
        }
    }
}

private extension Int {
    var nonZero: Int? { self == 0 ? nil : self }
}

struct RailButton: View {
    let icon: String
    let help: String
    let badge: Int?
    /// Warn for something wrong, accent for something available. A suggestion is an opportunity,
    /// and colouring it like a failure trains people to ignore the colour that means failure.
    var badgeTone: Color = K.C.accent
    let selected: Bool
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            // The fill comes first so the whole row is painted and therefore hit-testable:
            // a `.plain` button's target is otherwise the glyph itself. Full rail width, so the
            // selection bar can sit on the rail's own edge rather than 4pt inside it.
            Rectangle()
                .fill(hovering ? K.C.text.opacity(0.06) : .clear)
                .frame(width: 44, height: 32)
                // Centred, which is the whole point of the fixed frame. This was a
                // `ZStack(alignment: .topTrailing)` so the badge would sit in the corner — and
                // that alignment applied to the icon too, pushing every one of them right.
                .overlay(
                    Image(systemName: icon)
                        .font(.system(size: 14, weight: .regular))
                        .foregroundStyle(selected ? K.C.text : (hovering ? K.C.dim : K.C.faint))
                )
                // The badge is positioned on its own, so it cannot move the icon.
                .overlay(alignment: .topTrailing) {
                    if let badge {
                        Text("\(badge)")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundStyle(K.C.bg)
                            .padding(.horizontal, 3).padding(.vertical, 1)
                            .background(badgeTone, in: Capsule())
                            .offset(x: -4, y: 2)
                    }
                }
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .overlay(alignment: .leading) {
            if selected { Rectangle().fill(K.C.accent).frame(width: 2) }
        }
        .onHover { hovering = $0 }
        .hint(help)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

// MARK: - Status bar

/// The instrument readout. Everything here answers a question you would otherwise have to go and
/// ask: which branch, is this project trusted, what will the gate run, what has this cost.
struct StatusBar: View {
    let model: SessionModel
    @Binding var terminalOpen: Bool

    var body: some View {
        HStack(spacing: K.S.md) {
            ProjectMenu(model: model)
            if model.isRepo {
                item("arrow.triangle.branch", model.branch ?? "—")
            } else {
                HStack(spacing: 3) {
                    Image(systemName: "exclamationmark.triangle").font(.system(size: 10))
                    Text("no git").font(K.F.micro)
                }
                .foregroundStyle(K.C.warn)
                .help("This project is not a git repository. Initialise one from Changes.")
            }

            if model.trusted {
                HStack(spacing: 3) {
                    Image(systemName: "checkmark.shield.fill").font(.system(size: 10))
                    Text("trusted").font(K.F.micro)
                }
                .foregroundStyle(K.C.warn)
                .help("This project runs commands without asking. Withdraw in Settings › Permissions.")
            }

            if !model.changes.isEmpty {
                item("plusminus", "\(model.changes.count)")
            }

            Spacer()

            if let gate = model.gateCommand {
                item("checkmark.seal", gate)
            } else {
                HStack(spacing: 3) {
                    Image(systemName: "exclamationmark.triangle").font(.system(size: 10))
                    Text("no gate").font(K.F.micro)
                }
                .foregroundStyle(K.C.warn)
                .help("This project declares no checks, so Keel cannot verify a turn's claim.")
            }

            // The context window, in tokens. Warm past 150k because that is where compaction
            // starts to loom on a 200k model, and compaction you did not see coming is how a
            // four-hour session loses its file paths.
            if let ctx = model.contextTokens {
                HStack(spacing: 3) {
                    Image(systemName: "rectangle.stack").font(.system(size: 10))
                    Text("ctx \(compact(ctx))").font(K.F.mono(10)).monospacedDigit()
                }
                .foregroundStyle(ctx > 150_000 ? K.C.warn : K.C.faint)
                .help("Tokens in the context window after the last request. Compaction is near "
                      + "when this is high.")
            }
            if let t = model.sessionTokens {
                Text(compact(t.total) + " tok")
                    .font(K.F.mono(10)).monospacedDigit().foregroundStyle(K.C.faint)
                    .help("Tokens this session, cache included")
            }
            if model.running, let rate = model.burnRate {
                Text(String(format: "$%.2f/min", rate))
                    .font(K.F.mono(10)).monospacedDigit().foregroundStyle(K.C.faint)
                    .help("Spend rate, from this session's finished turns")
            }
            if let cost = model.sessionCost {
                Text(String(format: "$%.3f", cost))
                    .font(K.F.mono(10)).monospacedDigit().foregroundStyle(K.C.faint)
                    .help("This session, as reported by the CLI")
            }

            Button { withAnimation(K.M.quick) { terminalOpen.toggle() } } label: {
                Image(systemName: "terminal")
                    .font(.system(size: 10))
                    .frame(width: 22, height: 18)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(terminalOpen ? K.C.accent : K.C.faint)
            .hint("Terminal (⌘⌥T)")
        }
        .padding(.horizontal, K.S.md)
        .padding(.vertical, 5)
        .background(K.C.surface)
        .foregroundStyle(K.C.faint)
    }

    private func item(_ icon: String, _ text: String) -> some View {
        HStack(spacing: 3) {
            Image(systemName: icon).font(.system(size: 10))
            Text(text).font(K.F.mono(10)).lineLimit(1)
        }
    }
}


/// The open project, and every way of changing it.
///
/// The recents list used to exist only on the welcome screen, which you never see again once a
/// project is open — so switching meant a raw folder picker and remembering where things live.
struct ProjectMenu: View {
    let model: SessionModel
    /// The toolbar version: bigger, labelled, with the verb in it.
    var prominent = false

    var body: some View {
        Menu {
            Section("Recent") {
                ForEach(Recents.paths.filter { $0 != model.repoPath }, id: \.self) { path in
                    Button((path as NSString).lastPathComponent) {
                        Task {
                            try? await model.openProject(path)
                            Recents.remember(path)
                            await model.lanes?.refreshShared()
                        }
                    }
                }
            }
            Divider()
            Button("Open Another Project…   ⌘O") {
                NotificationCenter.default.post(name: .keelOpenProject, object: nil)
            }
            Button("New Project…   ⇧⌘N") {
                NotificationCenter.default.post(name: .keelNewProject, object: nil)
            }
            Divider()
            Button("Reveal in Finder") {
                NSWorkspace.shared.selectFile(nil, inFileViewerRootedAtPath: model.repoPath)
            }
        } label: {
            HStack(spacing: prominent ? 6 : 4) {
                Image(systemName: "folder.fill").font(.system(size: prominent ? 12 : 10))
                Text((model.repoPath as NSString).lastPathComponent)
                    .font(prominent ? K.F.ui(13, .semibold) : K.F.mono(10, .medium))
                Image(systemName: "chevron.down")
                    .font(.system(size: prominent ? 9 : 6, weight: .bold))
                if prominent {
                    Text("switch").font(K.F.micro).foregroundStyle(K.C.faint)
                }
            }
            .foregroundStyle(prominent ? K.C.text : K.C.dim)
            .padding(.horizontal, prominent ? K.S.sm : 0)
            .padding(.vertical, prominent ? 3 : 0)
            .background(prominent ? K.C.text.opacity(0.06) : .clear,
                        in: RoundedRectangle(cornerRadius: K.R.sm))
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .hint("Project: \(model.repoPath). Click to switch, open another, or start a new one (⌘O)")
    }
}


/// Everything the window reacts to.
///
/// Lifted out of the view body: SwiftUI type-checks a modifier chain as one expression, and twenty
/// `onChange`/`onReceive` in a row stops compiling in reasonable time long before it stops being
/// readable.
struct WindowEvents: ViewModifier {
    let lanes: Lanes
    let model: SessionModel
    @Binding var stage: SessionWindow.Stage
    @Binding var showSettings: Bool
    @Binding var showTerminal: Bool
    @Binding var paletteOpen: Bool
    @Binding var starting: Bool
    @Binding var panel: SessionWindow.Panel?
    @State private var confirmingTrust = false

    func body(content: Content) -> some View {
        content
            .modifier(ChatEvents(lanes: lanes, model: model, showSettings: $showSettings))
            .modifier(LaneEvents(lanes: lanes, panel: $panel))
            .onReceive(NotificationCenter.default.publisher(for: .keelTrust)) { _ in
                confirmingTrust = true
            }
            .modifier(TrustAlert(model: model, shown: $confirmingTrust))
            .onChange(of: lanes.waitingCount) { Notifications.badge(lanes.waitingCount) }
            .task(id: model.id) { await lanes.refreshShared() }
            .modifier(StageEvents(lanes: lanes, model: model, stage: $stage,
                                  showSettings: $showSettings))
            .onReceive(NotificationCenter.default.publisher(for: .keelPalette)) { _ in
                withAnimation(K.M.quick) { paletteOpen.toggle() }
            }
            .onReceive(NotificationCenter.default.publisher(for: .keelSettings)) { _ in
                withAnimation(K.M.quick) { showSettings.toggle() }
            }
            .onReceive(NotificationCenter.default.publisher(for: .keelNewLane)) { _ in
                lanes.newLane(isolated: true)
            }
            .onReceive(NotificationCenter.default.publisher(for: .keelNewProject)) { _ in
                starting = true
            }
            .onReceive(NotificationCenter.default.publisher(for: .keelToggleTerminal)) { _ in
                withAnimation(K.M.quick) { showTerminal.toggle() }
            }
            .onReceive(NotificationCenter.default.publisher(for: .keelOpenProject)) { _ in
                openProject()
            }
    }

    private func openProject() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.prompt = "Open"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        Task {
            try? await model.openProject(url.path)
            Recents.remember(url.path)
            await lanes.refreshShared()
        }
    }
}

/// ⌘⇧T. Trust is the highest-consequence switch in the app, so the keyboard path confirms and
/// names what it grants — the same words the approval card uses.
private struct TrustAlert: ViewModifier {
    let model: SessionModel
    @Binding var shown: Bool
    struct TrustBody: Encodable { var trusted: Bool }

    func body(content: Content) -> some View {
        content.alert("Trust this project?", isPresented: $shown) {
            Button("Trust") {
                Task {
                    _ = try? await model.client.post("/api/permissions/trust",
                                                     body: TrustBody(trusted: true), as: Bool.self)
                    await model.refreshTrust()
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The agent runs commands in this repository without asking. Stored in "
                 + ".keel/permissions.json; withdrawable from the status bar.")
        }
    }
}

/// ⌘1–9, ⌘⇧] / ⌘⇧[, ⌘⇧E, and a click on a banner. Its own modifier because the window's
/// chain of handlers had grown past what the type-checker finishes in reasonable time.
private struct LaneEvents: ViewModifier {
    let lanes: Lanes
    @Binding var panel: SessionWindow.Panel?

    func body(content: Content) -> some View {
        content
            .onReceive(NotificationCenter.default.publisher(for: .keelFocusLane)) { note in
                focus(note.object)
            }
            .onReceive(NotificationCenter.default.publisher(for: .keelNextLane)) { note in
                step((note.object as? Int) ?? 1)
            }
            .onReceive(NotificationCenter.default.publisher(for: .keelTogglePanel)) { _ in
                withAnimation(K.M.quick) { panel = panel == nil ? .changes : nil }
            }
    }

    /// Either a lane id (from a notification) or an index (from ⌘1–9).
    private func focus(_ object: Any?) {
        let all = lanes.lanes
        if let n = object as? Int {
            if n < all.count { lanes.activeID = all[n].id }
        } else if let raw = object as? String, let id = UUID(uuidString: raw),
                  all.contains(where: { $0.id == id }) {
            lanes.activeID = id
        }
        NSApp.activate()
    }

    private func step(_ by: Int) {
        let all = lanes.lanes
        guard !all.isEmpty, let i = all.firstIndex(where: { $0.id == lanes.activeID }) else { return }
        let n = (i + by + all.count) % all.count
        lanes.activeID = all[n].id
    }
}

/// ⌘↵ ⌘. ⌘L ⇧⇥ ⌘⇧A ⌘⇧D. Here rather than in `ChatRail`: that view is unmounted while Settings
/// is open, and with it went Stop, mid-turn.
private struct ChatEvents: ViewModifier {
    let lanes: Lanes
    let model: SessionModel
    @Binding var showSettings: Bool

    /// The lane a notification named, or the focused one.
    private func lane(named note: Notification) -> SessionModel {
        if let raw = note.object as? String, let id = UUID(uuidString: raw),
           let found = lanes.lanes.first(where: { $0.id == id }) {
            return found
        }
        return model
    }

    func body(content: Content) -> some View {
        content
            .onReceive(NotificationCenter.default.publisher(for: .keelSend)) { _ in model.send() }
            .onReceive(NotificationCenter.default.publisher(for: .keelStop)) { _ in model.stop() }
            .onReceive(NotificationCenter.default.publisher(for: .keelFocusComposer)) { _ in
                showSettings = false
                model.focusComposerTick += 1
            }
            .onReceive(NotificationCenter.default.publisher(for: .keelToggleMode)) { _ in
                model.mode = model.mode == "plan" ? "acceptEdits" : "plan"
            }
            .onReceive(NotificationCenter.default.publisher(for: .keelApprove)) { note in
                let m = lane(named: note)
                if let p = m.pending.first { m.answer(p, allow: true, scope: "session") }
            }
            .onReceive(NotificationCenter.default.publisher(for: .keelDeny)) { note in
                let m = lane(named: note)
                if let p = m.pending.first { m.answer(p, allow: false, scope: "session") }
            }
    }
}

/// The divider between the conversation and the record, as something you can drag.
///
/// The stage was a fixed ideal width, which is right on the first day and wrong on the second:
/// reviewing a wide diff wants the record wide, and a long conversation wants it narrow. The
/// handle is 7pt of hit area drawn as a 1pt line; double-click puts it back.
private struct SplitHandle: View {
    @Binding var width: Double
    let range: ClosedRange<Double>
    let reset: Double
    @State private var hovering = false
    @State private var start: Double?

    var body: some View {
        Rectangle()
            .fill(hovering ? K.C.accent.opacity(0.6) : K.C.line)
            .frame(width: 1)
            .padding(.horizontal, 3)
            .contentShape(Rectangle())
            .onHover { hovering = $0; if $0 { NSCursor.resizeLeftRight.push() } else { NSCursor.pop() } }
            .gesture(
                DragGesture(minimumDistance: 1)
                    .onChanged { g in
                        if start == nil { start = width }
                        // The stage is on the right, so dragging left makes it wider.
                        width = min(max((start ?? width) - g.translation.width, range.lowerBound),
                                    range.upperBound)
                    }
                    .onEnded { _ in start = nil }
            )
            .onTapGesture(count: 2) { width = reset }
            .accessibilityLabel("Resize the record")
    }
}

/// What the right pane shows, following the work: a preview URL appearing, the agent editing
/// the page, a turn starting, a turn being picked in the conversation.
private struct StageEvents: ViewModifier {
    let lanes: Lanes
    let model: SessionModel
    @Binding var stage: SessionWindow.Stage
    @Binding var showSettings: Bool

    func body(content: Content) -> some View {
        content
            .onChange(of: model.previewURL) { showPreviewIfIdle() }
            .onChange(of: model.designTick) { followTheEdit() }
            .onChange(of: model.running) { followTheWork() }
            .onChange(of: model.focusedTurn) { showTrace() }
    }

    /// Something to look at is worth looking at — unless you are deliberately reading something
    /// else.
    private func showPreviewIfIdle() {
        guard model.previewURL != nil, !showSettings,
              model.viewingDiff == nil, model.inspecting == nil,
              model.focusedTurn == nil else { return }
        withAnimation(K.M.quick) { stage = .preview }
    }

    /// The agent is writing the page. Show the page — that is the whole point of having one.
    private func followTheEdit() {
        guard model.followEdits, model.editing != nil, model.previewURL != nil,
              model.id == lanes.activeID else { return }
        withAnimation(K.M.quick) {
            stage = .preview
            showSettings = false
            model.viewingDiff = nil
            model.inspecting = nil
        }
    }

    /// Starting a turn means the record is the thing to look at. Never off the Designer: picking
    /// an element and typing what to do about it is one gesture.
    private func followTheWork() {
        guard model.running, model.id == lanes.activeID, stage != .preview else { return }
        stage = .turn
        showSettings = false
        model.viewingDiff = nil
        model.inspecting = nil
        model.focusedTurn = nil
    }

    private func showTrace() {
        guard model.focusedTurn != nil else { return }
        stage = .turn
        showSettings = false
        model.viewingDiff = nil
        model.inspecting = nil
    }

}
