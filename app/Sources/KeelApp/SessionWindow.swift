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
    var app: AppModel? = nil
    /// History, not Changes. A window opens onto a project you have worked in before, and the
    /// first question is which conversation to carry on — Changes is empty until a turn runs.
    /// The window this is, so a menu key acts in one window rather than in all of them.
    @State private var here = WindowHere()
    @State private var panel: Panel? = .sessions
    @State private var stage: Stage = .turn
    /// A turn ran while the person was looking at something else. The Trace tab keeps moving until
    /// they look at it — see `stageBar`.
    @State private var traceUnread = false
    @State private var showTerminal = false
    @State private var terminalTitle = "shell"
    @State private var terminalCommand: String?
    @State private var paletteOpen = false
    @State private var starting = false
    @State private var startingFeature = false
    @State private var showSettings = false
    /// How wide the record beside the conversation is. Yours to drag; remembered.
    @AppStorage("keel.stageWidth") private var stageWidth: Double = 460
    /// And the side panel.
    @AppStorage("keel.panelWidth") private var panelWidth: Double = 256

    /// The two things the right pane can be.
    ///
    /// "Trace" rather than "Turn": the pane is the record of what the agent did, and a turn is the
    /// unit inside it. "Designer" rather than "Preview": you do not only look at the page there,
    /// you pick things in it and change them.
    ///
    /// Trace is first, and the one a new window opens on: the centre of a window is the turn, and
    /// the readiness report is something you go and ask for.
    enum Stage: String, CaseIterable {
        case turn = "Trace", review = "Review", preview = "Designer"

        /// The tabs a window offers. The Designer is behind `Flags.designer`, and hiding it means
        /// hiding the tab *and* the two redirects that take the stage there on their own — a stage
        /// nothing can navigate to is only half-hidden if something still switches to it.
        static var shown: [Stage] { Flags.designer ? allCases : [.turn, .review] }
    }

    /// One icon per thing, because they are different things. Grouping skills, subagents, MCP
    /// servers, hooks and plugins into one "Workspace" panel meant five headings fighting for a
    /// 256pt rail and no room for any of them to have actions.
    enum Panel: String, CaseIterable, Identifiable {
        // The rail is drawn in this order, so it is written in the order people reach for it:
        // what you were just doing, what changed, what git makes of it, and what is still running.
        // The reference panels come after the divider and are read far less often.
        case sessions, changes, git, monitors
        case files, readiness
        case skills, agents, mcp, hooks, plugins
        var id: String { rawValue }

        /// The panels the rail and the palette offer. Readiness is behind `Flags.readiness`.
        static var shown: [Panel] { allCases.filter { $0 != .readiness || Flags.readiness } }

        var icon: String {
            switch self {
            case .changes: "plusminus"
            case .git: "arrow.triangle.branch"
            case .files: "folder"
            case .sessions: "clock.arrow.circlepath"
            case .readiness: "checkmark.shield"
            case .monitors: "binoculars"
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
            case .git: "Git"
            case .files: "Files"
            case .sessions: "History"
            case .readiness: "Readiness"
            case .monitors: "Monitors"
            case .skills: "Skills"
            case .agents: "Subagents"
            case .mcp: "MCP"
            case .hooks: "Hooks"
            case .plugins: "Plugins"
            }
        }

        /// A separator after these, so the repository group and the agent-configuration group read
        /// as two sets rather than nine icons in a column.
        /// The divider sits after the panels about *this* project's work, before the ones that
        /// are reference material about the machine's setup.
        /// Readiness is the last of the project group, so with it hidden the divider moves up to
        /// Files rather than vanishing and leaving nine icons in one column.
        var endsGroup: Bool { self == (Flags.readiness ? .readiness : .files) }
    }

    /// The lane in focus. Every pane below draws this one; the rail shows all of them.
    private var model: SessionModel { lanes.active }

    var body: some View {
        Group {
            // `atStart` first, and short-circuiting: the person closed every tab, and the daemon
            // still has the project open, so `projectOpen` would put the workbench straight back.
            if lanes.atStart {
                Welcome(model: model, daemonFailure: app?.failure) { lanes.atStart = false }
            } else if model.projectOpen {
                workbench
            } else {
                Welcome(model: model, daemonFailure: app?.failure) { }
            }
        }
        .animation(K.M.quick, value: model.opening)
        .animation(K.M.quick, value: model.justOpened)
        .animation(K.M.quick, value: model.loaded)
        .background(K.C.bg)
        .windowCommands(here)
        .task {
            await lanes.refreshShared()
            // Restore once the project is known: the saved lanes are keyed by it.
            await lanes.restore(repo: model.repoPath)
        }
        // A different project is a different set of conversations. Without this the old
        // project's tabs stayed in the strip and were then saved under the new project's name.
        //
        // The first `"" → path` is not a switch, it is the cold launch finishing: `repoPath` is
        // only filled in when `/api/state` answers, which is after the `.task` above has already
        // called `restore` with an empty repo and had it return on its own guard. Skipping this
        // transition meant a relaunch restored nothing — every lane gone, back to one empty tab.
        .onChange(of: model.repoPath) { was, now in
            guard was != now, !now.isEmpty else { return }
            Task {
                if was.isEmpty { await lanes.restore(repo: now) }
                else { await lanes.switchProject(to: now) }
            }
        }
        // Anything that changes which conversations are open is worth writing down: a lane that
        // has just been given a session id, one closed, or a different one focused.
        .onChange(of: lanes.lanes.compactMap(\.sessionId)) {
            lanes.remember(repo: model.repoPath)
        }
        .onChange(of: lanes.activeID) {
            lanes.remember(repo: model.repoPath)
        }
        // The working tree also changes when Keel is not the one changing it — a commit in the
        // terminal, a revert in another lane, an editor saving a file. Nothing re-read git status
        // between turns, so the Changes panel kept listing files that were no longer changed and
        // clicking one opened a diff of nothing. Coming back to the window is the moment to look.
        .onReceive(NotificationCenter.default.publisher(
            for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { await model.refreshGit(); await model.refreshTree() }
        }
    }

    private var workbench: some View {
        VStack(spacing: 0) {
            if let app, !app.crashes.isEmpty {
                CrashBar(reports: app.crashes) { Crashes.markSeen(); app.crashes = [] }
                Hairline()
            }
            // The lanes are always on screen, across the top. They are the reason this is a
            // window and not a terminal: several agents working at once, and you can see all
            // of them.
            LaneTabs(lanes: lanes)
            Hairline()
            if let o = model.opening {
                OpeningBar(name: o.name, stage: o.stage, done: false)
                    .transition(.move(edge: .top).combined(with: .opacity))
            } else if let name = model.justOpened {
                OpeningBar(name: name, stage: "\(model.changes.count) changed · \(model.findings.count) findings", done: true)
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
            HStack(spacing: 0) {
                ActivityRail(panel: $panel, model: model, onSettings: {
                    withAnimation(K.M.quick) { showSettings.toggle() }
                }, onPanel: {
                    // Picking a panel is leaving Settings, whatever the gear is showing.
                    withAnimation(K.M.quick) { showSettings = false }
                })

                if showsPanel, let panel {
                    SidePanel(panel: panel, model: model) {
                        withAnimation(K.M.quick) { self.panel = nil }
                    }
                    .frame(width: panelFit)
                    SplitHandle(width: $panelWidth, range: 200...520, reset: 256, leading: true)
                }

                if showSettings {
                    SettingsPage(model: model, pairing: pairing) {
                        withAnimation(K.M.quick) { showSettings = false }
                    }
                    .frame(maxWidth: .infinity)
                } else {
                    working
                }
            }
            .onGeometryChange(for: Double.self) { $0.size.width } action: { available = $0 }
            // Dimmed while a switch is in flight, so the old project's content reads as
            // "going away" rather than as the new one.
            .opacity(model.opening == nil ? 1 : 0.45)
            // Full width, under both panes: a terminal is where a build's output goes, and a
            // build's output is wider than the composer.
            if showTerminal {
                Hairline()
                terminalPane
            }
            Hairline()
            StatusBar(model: model, terminalOpen: $showTerminal) {
                NotificationCenter.default.post(name: .keelTrust, object: nil)
            }
        }
        .overlay(alignment: .top) { paletteOverlay }
        .toolbar { toolbar }
        .navigationTitle((model.repoPath as NSString).lastPathComponent.isEmpty
                         ? "Keel" : (model.repoPath as NSString).lastPathComponent)
        .navigationSubtitle(subtitle)
        .sheet(isPresented: $startingFeature) {
            NewFeature(model: model, lanes: lanes) { startingFeature = false }
        }
        // Every sheet the panels open, anchored on the window rather than on `SidePanel`.
        //
        // `SidePanel` leaves the hierarchy when the panel is collapsed (⌘⇧E), and a sheet attached
        // to a view that is not in the hierarchy simply never appears: ⌘K → "Project setup" with
        // the panel closed set the state and drew nothing. The window is always there.
        .sheet(item: Binding(get: { model.sheet }, set: { model.sheet = $0 })) { which in
            switch which {
            case .pr:
                PullRequest(model: model) { model.sheet = nil }
            case .skills:
                SkillCatalog(client: model.client) {
                    model.sheet = nil
                    Task { await model.refreshState(); await model.refreshSuggestions() }
                }
            case .subagent:
                NewSubagent(client: model.client) {
                    model.sheet = nil
                    Task { await model.refreshState() }
                }
            case .mcp:
                AddMCP(client: model.client) {
                    model.sheet = nil
                    Task { await model.refreshState() }
                }
            case .setup:
                SetupSheet(model: model) { model.sheet = nil }
            }
        }
        .onWindowCommand(.keelNewFeatureSheet) { _ in
            // Only the window you are in: a sheet in every window at once is a modal maze.
            startingFeature = true
        }
        // A turn arriving while you are on the Designer or the readiness report is the case where
        // the record is written and never read. Marked unread there, and read the moment the Trace
        // is on the stage — however you got there, clicking the tab included.
        .onChange(of: model.turns.count) { if stage != .turn { traceUnread = true } }
        .onChange(of: stage) { if stage == .turn { traceUnread = false } }
        .onChange(of: model.id) { traceUnread = false }
        .modifier(WindowEvents(
            lanes: lanes, model: model,
            stage: $stage, showSettings: $showSettings, showTerminal: $showTerminal, terminalCommand: $terminalCommand,
            paletteOpen: $paletteOpen, starting: $starting, panel: $panel))
        .sheet(isPresented: $starting) {
            StartProject(client: model.client) { path, brief, file in
                starting = false
                Task {
                    await model.open(project: path)
                    Recents.remember(path)
                    await lanes.refreshShared()
                    // A template's brief goes into the box, its file onto the strip, and the
                    // first turn starts — "new project" ends with the agent working.
                    if let file { model.attach(fileURL: file) }
                    if !brief.isEmpty {
                        model.prompt = brief
                        try? await Task.sleep(for: .milliseconds(file == nil ? 100 : 1200))
                        model.send()
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var paletteOverlay: some View {
        if paletteOpen {
            Palette(model: model, open: $paletteOpen)
                .padding(.top, K.S.xxl + K.S.xl)
                .transition(.scale(scale: 0.98).combined(with: .opacity))
        }
    }

    /// The working area: conversation in the middle, the record beside it.
    private var working: some View {
        HStack(spacing: 0) {
            // The conversation is the middle, because it is what you are doing. The record of what
            // the agent changed is a reference you consult, so it sits beside it.
            // Keyed to the lane. Without this the view's own `@State` — the scroll anchor
            // above all — survives a lane swap, and `scrollPosition(id:)` is then held against
            // a turn id belonging to the conversation you just left. An id that resolves to
            // nothing scrolls into empty space, which is the blank pane people report.
            ChatRail(model: model)
                .id(model.id)
                .frame(minWidth: 360)

            if showsStage {
            SplitHandle(width: $stageWidth, range: 300...720, reset: 460)

            VStack(spacing: 0) {
                stageBar
                Hairline()
                detourBar
                Group {
                    if let target = model.inspecting {
                        Inspector(model: model, target: target)
                    } else if let commit = model.viewingCommit {
                        CommitSurface(model: model, commit: commit)
                    } else if let file = model.viewingFile {
                        FileSurface(model: model, path: file)
                    } else if let path = model.viewingDiff {
                        DiffSurface(model: model, path: path)
                    } else {
                        switch stage {
                        case .review: ReviewPacketView(model: model, lanes: lanes)
                        case .turn: TurnStage(model: model).id(model.id)
                        case .preview: PreviewSurface(model: model)
                        }
                    }
                }
            }
            .frame(width: stageFit)
            }
        }
    }

    // MARK: - Making the columns fit the window

    /// The width of the row the columns live in, or 0 before it has been laid out.
    ///
    /// `onGeometryChange` rather than a `GeometryReader`: it reports the size without taking part
    /// in layout, so there is no proposal to divide by and nothing to draw at zero.
    @State private var available: Double = 0

    /// Everything in the row that is not one of the two resizable columns.
    static let railWidth: Double = 60
    static let handleWidth: Double = 9
    static let chatMinWidth: Double = 360
    static let stageMinWidth: Double = 300
    static let panelMinWidth: Double = 200

    /// Below this there is no room for the side panel beside a conversation and a stage.
    static let panelFloor = railWidth + panelMinWidth + handleWidth
        + chatMinWidth + handleWidth + stageMinWidth
    /// Below this there is no room for the stage either, and the window is a conversation.
    static let stageFloor = railWidth + chatMinWidth + handleWidth + stageMinWidth

    /// Whether each column has the room to be drawn at all.
    ///
    /// Two Keel windows side by side on a 14" MacBook Pro is 756pt each, which is less than the
    /// three columns need however hard they are squeezed — so the panel steps aside rather than
    /// the window refusing to be that narrow. `panel` itself is untouched, so it comes back the
    /// moment there is room, and ⌘⇧E still works.
    private var showsPanel: Bool {
        panel != nil && (available == 0 || available >= Self.panelFloor)
    }
    private var showsStage: Bool { available == 0 || available >= Self.stageFloor }

    /// What is left for the panel and the stage once the rail, the handles and the conversation
    /// have taken theirs.
    private var roomForColumns: Double {
        var fixed = Self.railWidth + Self.chatMinWidth
        if showsPanel { fixed += Self.handleWidth }
        if showsStage { fixed += Self.handleWidth }
        return available - fixed
    }

    /// The stored widths, clamped to what the window can actually show.
    ///
    /// Both columns were rigid `.frame(width:)` reading straight from `@AppStorage`, and nothing
    /// checked them against the window. Dragged out to a 299pt panel and a 720pt stage, the row
    /// needed 1517pt — wider than the whole 1512pt screen of a 14" MacBook Pro — so the activity
    /// rail was pushed off the left edge and there was no way to get it back.
    ///
    /// The stored width is a preference, not a measurement: it is left alone, and only what gets
    /// drawn is clamped. Widen the window and the pane you asked for comes back.
    private var panelFit: Double {
        guard available > 0 else { return panelWidth }
        return Self.columns(in: available, panel: panelWidth, stage: stageWidth,
                            wantsPanel: panel != nil).panel
    }

    private var stageFit: Double {
        guard available > 0 else { return stageWidth }
        return Self.columns(in: available, panel: panelWidth, stage: stageWidth,
                            wantsPanel: panel != nil).stage
    }

    /// What the two columns actually get, given the room and what was asked for.
    ///
    /// Pure and static so the invariant can be checked without a window: for every width and every
    /// pair of stored widths, what is drawn has to *fit*. It did not, and the overflow came off the
    /// left and took the activity rail with it — a 299pt panel and a 720pt stage need 1517pt of
    /// row on a screen 1512pt wide.
    ///
    /// A column of zero means it steps aside: below `panelFloor` there is no room for the panel
    /// beside a conversation and a stage, and below `stageFloor` there is no room for the stage.
    static func columns(in available: Double, panel: Double, stage: Double, wantsPanel: Bool)
        -> (panel: Double, stage: Double)
    {
        let showsPanel = wantsPanel && available >= panelFloor
        let showsStage = available >= stageFloor
        var fixed = railWidth + chatMinWidth
        if showsPanel { fixed += handleWidth }
        if showsStage { fixed += handleWidth }
        let room = available - fixed

        let p = showsPanel
            ? max(panelMinWidth, min(panel, room - (showsStage ? stageMinWidth : 0)))
            : 0
        let s = showsStage ? max(stageMinWidth, min(stage, room - p)) : 0
        return (p, s)
    }

    private var subtitle: String {
        let branch = model.branch ?? ""
        return model.trusted ? (branch.isEmpty ? "trusted" : branch + " · trusted") : branch
    }

    /// A segmented control drawn by hand: the stock one is a rounded capsule that reads as iOS.
    private var stageBar: some View {
        HStack(spacing: K.S.xxs) {
            ForEach(Stage.shown, id: \.self) { s in
                // Selected even while a diff or a file is covering the stage. It used to go dark
                // for all four of those, so opening a diff put you somewhere the navigation could
                // not describe — no tab lit, no name for where you were, no way back but an ✕.
                // The tab is where you came from; `detourBar` below says where you have gone.
                let on = stage == s
                // A turn ran somewhere you were not looking — nearly always because the Designer
                // took the stage — and the record of it sits behind a tab nobody clicked. The dot
                // keeps pulsing until you do, rather than blinking once and going still.
                let unread = s == .turn && traceUnread && !on
                let active = (tabActivity(for: s) || unread) && !on
                HStack(spacing: K.S.half) {
                    Image(systemName: stageIcon(s)).font(K.F.micro.weight(.medium))
                    Text(s.rawValue).font(K.F.small.weight(on ? .semibold : .regular))
                    if active {
                        Circle()
                            .fill(K.C.accent)
                            .frame(width: 5, height: 5)
                            .transition(.scale.combined(with: .opacity))
                            .phaseAnimator([false, true]) { dot, pulse in
                                dot.opacity(pulse ? 0.35 : 1)
                                    .scaleEffect(pulse ? 0.78 : 1)
                            } animation: { _ in .easeInOut(duration: 0.8) }
                    }
                }
                    .foregroundStyle(on ? K.C.text : K.C.dim)
                    .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
                    .background(on ? K.C.raised : .clear, in: RoundedRectangle(cornerRadius: K.R.md))
                    .overlay {
                        if on { RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line) }
                    }
                    .modifier(Nudge(active: unread))
                    .contentShape(Rectangle())
                    .asButton {
                        stage = s
                        closeDetour()
                        Telemetry.breadcrumb("stage: \(s.rawValue)")
                    }
                    .accessibilityAddTraits(on ? .isSelected : [])
            }
            Spacer()
        }
        .padding(.horizontal, K.S.md)
        .padding(.vertical, K.S.half)
        .background(K.C.surface)
    }

    /// What is covering the stage, if anything: the four surfaces the segmented control has no
    /// place for. Each is reached by clicking something in a panel, and each used to be a dead end.
    private var detour: (icon: String, title: String)? {
        if model.inspecting != nil { return ("scope", "Picked element") }
        if let c = model.viewingCommit { return ("checkmark.seal", c.subject) }
        if let f = model.viewingFile { return ("doc", (f as NSString).lastPathComponent) }
        if let d = model.viewingDiff { return ("plusminus", (d as NSString).lastPathComponent) }
        return nil
    }

    private func closeDetour() {
        model.viewingDiff = nil
        model.inspecting = nil
        model.viewingCommit = nil
        model.viewingFile = nil
    }

    /// Where you are, and the way back.
    ///
    /// ⌘[ rather than Esc: Esc is Stop while a turn is running, and that is the binding that must
    /// not be ambiguous.
    @ViewBuilder
    private var detourBar: some View {
        if let d = detour {
            HStack(spacing: K.S.half) {
                Button { closeDetour() } label: {
                    HStack(spacing: K.S.xs) {
                        Image(systemName: "chevron.left").font(K.F.tiny.weight(.semibold))
                        Text(stage.rawValue).font(K.F.small)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain).foregroundStyle(K.C.accent)
                .keyboardShortcut("[", modifiers: .command)
                .hint("Back to \(stage.rawValue) (⌘[)")

                Image(systemName: "chevron.right").font(K.F.tiny).foregroundStyle(K.C.faint)
                Image(systemName: d.icon).font(K.F.tiny).foregroundStyle(K.C.faint)
                Text(d.title).font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                    .lineLimit(1).truncationMode(.head)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.half)
            .background(K.C.surface)
            .overlay(alignment: .bottom) { Hairline() }
        }
    }

    private func stageIcon(_ stage: Stage) -> String {
        switch stage {
        case .review: "checkmark.shield"
        case .turn: "list.bullet.rectangle"
        case .preview: "cursorarrow.motionlines"
        }
    }

    /// The tab is unread and the person is somewhere else: a short jiggle, a long pause, repeat.
    /// It runs only while `active`, because a control that moves forever is one you stop seeing.
    private struct Nudge: ViewModifier {
        let active: Bool

        func body(content: Content) -> some View {
            if active {
                content.phaseAnimator([0.0, -2.5, 2.5, 0.0]) { tab, x in
                    tab.offset(x: x)
                } animation: { x in .easeInOut(duration: x == 0 ? 1.4 : 0.12) }
            } else {
                content
            }
        }
    }

    private func tabActivity(for stage: Stage) -> Bool {
        switch stage {
        case .turn: return model.running
        case .preview: return model.editing != nil
        case .review: return model.running && model.lastReview != nil
        }
    }

    private var terminalPane: some View {
        VStack(spacing: 0) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "terminal").font(K.F.tiny).foregroundStyle(K.C.faint)
                Text(terminalTitle).font(K.F.codeTiny).foregroundStyle(K.C.dim)
                Spacer()
                CloseButton(size: 10, label: "Close the terminal") {
                    withAnimation(K.M.quick) { showTerminal = false }
                }
            }
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.xs)
            .background(K.C.surface)
            // `.id(model.id)` tears the pane down on every lane switch, and the daemon spawns a
            // fresh PTY per connection — so switching away and back gives a new shell with empty
            // scrollback and whatever was running gone. Keying on the *checkout* instead means a
            // switch between lanes on one tree keeps its shell, which is the common case; the
            // header says what happened when it genuinely could not.
            TerminalPane(port: model.port, worktree: model.worktree, title: $terminalTitle, command: $terminalCommand)
                .id(model.worktree ?? model.repoPath)
        }
        .frame(minHeight: 140, idealHeight: 220)
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .primaryAction) {
            AppearanceMenu()
        }
        ToolbarItem(placement: .primaryAction) {
            Button { withAnimation(K.M.quick) { paletteOpen.toggle() } } label: {
                Image(systemName: "command")
            }
            .hint("Command palette (⌘K)")
        }
        // No terminal button here. There was one in the toolbar *and* one in the status bar, same
        // glyph, same shortcut, both on screen at once — and the status bar's sits beside the pane
        // it opens, which is where a toggle belongs.
    }
}

// MARK: - Activity rail

/// Icon-only, 44pt, with a selection bar. The panel it opens is the one you were last in, and
/// clicking the active icon collapses the panel — the two gestures every editor has.
struct ActivityRail: View {
    @Binding var panel: SessionWindow.Panel?
    let model: SessionModel
    var onSettings: () -> Void = {}
    var onPanel: () -> Void = {}

    var body: some View {
        VStack(alignment: .center, spacing: K.S.xxs) {
            ForEach(SessionWindow.Panel.shown) { p in
                RailButton(
                    icon: p.icon,
                    label: p.title,
                    help: help(p),
                    badge: badge(p),
                    badgeTone: p == .hooks || p == .readiness || p == .plugins ? K.C.warn : K.C.accent,
                    selected: panel == p
                ) {
                    withAnimation(K.M.quick) { panel = (panel == p) ? nil : p }
                    onPanel()
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
                VStack(spacing: K.S.tight) {
                    Image(systemName: "gearshape").font(K.F.ui(17))
                    Text("Settings").font(K.F.tiny)
                }
                    .foregroundStyle(toolsNeedAttention > 0 ? K.C.warn : K.C.faint)
                    .frame(width: 60, height: 46)
                    .overlay(alignment: .topTrailing) {
                        if toolsNeedAttention > 0 {
                            Text("\(toolsNeedAttention)")
                                .font(K.F.tiny.weight(.bold))
                                .foregroundStyle(K.C.bg)
                                .padding(.horizontal, K.S.tight).padding(.vertical, K.S.hair)
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
        // Fixed and first in line for space: the rail never gives up width to a pane being
        // dragged beside it.
        .frame(width: 60)
        .fixedSize(horizontal: true, vertical: false)
        .layoutPriority(2)
        .background(K.C.surface)
        .overlay(alignment: .trailing) { Rectangle().fill(K.C.line).frame(width: 1) }
    }

    /// The tooltip carries the reason for the badge, so a number on an icon is not a riddle.
    private func help(_ p: SessionWindow.Panel) -> String {
        let n = model.missingSuggestions.count
        switch p {
        case .plugins where n > 0:
            return "Plugins — \(n) recommended for this repository, not installed"
        case .skills:
            return "Skills — \(model.workspace.skills.count) installed"
        case .monitors:
            let n = model.monitors.count(where: \.running)
            return n > 0 ? "Monitors — \(n) running in the background" : p.title
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
        // What the panel behind it actually lists. It counted every uncommitted file in the
        // repository, so a fresh conversation in a dirty checkout wore a 13 over an empty panel
        // saying "the agent has not written a file in this conversation" — both true, about
        // different things.
        case .changes: model.editedThisSession.count.nonZero
        // Git keeps the other number, because Git's own list *is* the uncommitted files: it is
        // the count of what a commit there would take.
        case .git: model.changes.count.nonZero
        // Every finding, not only the blocking ones: a warning in the panel with no number on
        // the icon read as a panel that had nothing to say.
        case .readiness: model.findings.count.nonZero
        // Only what is still going. A finished job is history the moment it is reported, and a
        // number that never goes back down is a number people stop reading.
        case .monitors: model.monitors.count(where: \.running).nonZero
        // Hooks that came with the repository are the one count worth shouting: each is a shell
        // command someone else wrote that runs on this machine.
        case .hooks: model.workspace.hooks.filter(\.fromRepo).count.nonZero
        // What this repository is missing, on the panel that installs it. Skills get no number
        // of their own: a plugin is what carries them, and the same count on two icons read as
        // one bug.
        case .plugins: model.missingSuggestions.count.nonZero
        default: nil
        }
    }
}

private extension Int {
    var nonZero: Int? { self == 0 ? nil : self }
}

struct RailButton: View {
    let icon: String
    /// Under the icon. Nine unlabelled glyphs in a column is a riddle; testers said so.
    var label: String? = nil
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
                .fill(hovering ? K.C.hover : .clear)
                .frame(width: 60, height: 46)
                // Centred, which is the whole point of the fixed frame. This was a
                // `ZStack(alignment: .topTrailing)` so the badge would sit in the corner — and
                // that alignment applied to the icon too, pushing every one of them right.
                .overlay(
                    VStack(spacing: K.S.tight) {
                        Image(systemName: icon).font(K.F.ui(17, .regular))
                        if let label {
                            Text(label).font(K.F.tiny).lineLimit(1)
                                .minimumScaleFactor(0.8)
                        }
                    }
                    .foregroundStyle(selected ? K.C.text : (hovering ? K.C.dim : K.C.faint))
                )
                // The badge is positioned on its own, so it cannot move the icon.
                .overlay(alignment: .topTrailing) {
                    if let badge {
                        Text("\(badge)")
                            .font(K.F.tiny.weight(.bold))
                            .foregroundStyle(K.C.bg)
                            .padding(.horizontal, K.S.tight).padding(.vertical, K.S.hair)
                            .background(badgeTone, in: Capsule())
                            .offset(x: -8, y: 3)
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
    var onTrust: (() -> Void)? = nil
    @State private var branchMenu = false

    var body: some View {
        HStack(spacing: K.S.md) {
            if model.isWorkspace {
                // Several repositories in one folder. Each keeps its own branch, and there is no
                // single one to name — so they are all named, which is also how you tell at a
                // glance that this folder is a workspace rather than a project.
                ForEach(model.repos) { repo in
                    HStack(spacing: K.S.tight) {
                        Image(systemName: "arrow.triangle.branch").font(K.F.tiny)
                        Text(repo.name).font(K.F.micro).foregroundStyle(K.C.dim)
                        Text(repo.branch ?? "—").font(K.F.codeTiny)
                    }
                    .foregroundStyle(K.C.faint)
                    .help("\(repo.name) is its own git repository, on \(repo.branch ?? "no branch")")
                }
            } else if model.isRepo {
                // The branch is a menu, the way every IDE's status bar treats it: click to see
                // the others and switch, or start a new one from here.
                // The branch is a menu, the same as every IDE's status bar — and it was drawn as
                // one more grey readout, so nothing said so.
                StatusToggle(icon: "arrow.triangle.branch", title: model.branch ?? "—",
                             on: branchMenu, shortcut: "switch or create a branch") {
                    Task { await model.refreshBranches() }
                    branchMenu = true
                }
                .popover(isPresented: $branchMenu, arrowEdge: .top) {
                    BranchMenu(model: model) { branchMenu = false }
                }
            } else {
                HStack(spacing: K.S.tight) {
                    Image(systemName: "exclamationmark.triangle").font(K.F.tiny)
                    Text("no git").font(K.F.micro)
                }
                .foregroundStyle(K.C.warn)
                .help("This project is not a git repository. Initialise one from Changes.")
            }

            if model.trusted {
                HStack(spacing: K.S.tight) {
                    Image(systemName: "checkmark.shield.fill").font(K.F.tiny)
                    Text("trusted").font(K.F.micro)
                }
                .foregroundStyle(K.C.warn)
                .help("This project runs commands without asking. Withdraw in Settings › Permissions.")
            } else if model.projectOpen {
                // Untrusted is the safe state, but it is also the one where every command
                // becomes a question — said here so the first refusal is not a surprise, and
                // one click away from the decision that removes the toll.
                HStack(spacing: K.S.tight) {
                    Image(systemName: "exclamationmark.shield").font(K.F.tiny)
                    Text("not trusted · commands will ask").font(K.F.micro)
                }
                .foregroundStyle(K.C.warn)
                .contentShape(Rectangle())
                .asButton { onTrust?() }
                .help("Commands the agent runs here need your approval each time. Click to trust this project (⌘⇧T).")
            }

            if !model.changes.isEmpty {
                item("plusminus", "\(model.changes.count)")
            }

            Spacer()

            if let gate = model.gateCommand {
                item("checkmark.seal", gate)
            } else {
                HStack(spacing: K.S.tight) {
                    Image(systemName: "exclamationmark.triangle").font(K.F.tiny)
                    Text("no gate").font(K.F.micro)
                }
                .foregroundStyle(K.C.warn)
                .help("This project declares no checks, so Keel cannot verify a turn's claim.")
            }

            // The context window, in tokens. Warm past 150k because that is where compaction
            // starts to loom on a 200k model, and compaction you did not see coming is how a
            // four-hour session loses its file paths.
            if let ctx = model.contextTokens {
                HStack(spacing: K.S.tight) {
                    Image(systemName: "rectangle.stack").font(K.F.tiny)
                    Text("ctx \(compact(ctx))").font(K.F.codeTiny).monospacedDigit()
                }
                .foregroundStyle(ctx > 150_000 ? K.C.warn : K.C.faint)
                .help("Tokens in the context window after the last request. Compaction is near "
                      + "when this is high.")
            }
            if let t = model.sessionTokens {
                Text(compact(t.total) + " tok")
                    .font(K.F.codeTiny).monospacedDigit().foregroundStyle(K.C.faint)
                    .help("Tokens this session, cache included")
            }
            if model.running, let rate = model.burnRate {
                Text(money(rate, places: 2) + "/min")
                    .font(K.F.codeTiny).monospacedDigit().foregroundStyle(K.C.faint)
                    .help("Spend rate, from this session's finished turns")
            }
            if let cost = model.sessionCost {
                Text(money(cost))
                    .font(K.F.codeTiny).monospacedDigit().foregroundStyle(K.C.faint)
                    .help("This session, as reported by the CLI")
            }

            StatusToggle(icon: "terminal", title: "Terminal",
                         on: terminalOpen, shortcut: "⌘⌥T") {
                withAnimation(K.M.quick) { terminalOpen.toggle() }
            }
        }
        .padding(.horizontal, K.S.md)
        .padding(.vertical, K.S.snug)
        .background(K.C.surface)
        .foregroundStyle(K.C.faint)
    }

    private func item(_ icon: String, _ text: String) -> some View {
        HStack(spacing: K.S.tight) {
            Image(systemName: icon).font(K.F.tiny)
            Text(text).font(K.F.codeTiny).lineLimit(1)
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

    /// A feature checkout nobody has a tab on, being thrown away.
    @State private var discarding: Wire.Worktree?
    @State private var refusal: String?

    var body: some View {
        Menu {
            // Before Recent, because these are this project's and they are the ones costing
            // something. A closed tab leaves its branch and its checkout on disk on purpose, and
            // until now nothing ever said which ones — so they accumulated, gigabytes each,
            // findable only with `git worktree list`.
            if let lanes = model.lanes, !lanes.orphans.isEmpty {
                Section("Features with no tab") {
                    ForEach(lanes.orphans) { checkout in
                        Menu(Self.describe(checkout)) {
                            Button("Reopen in a tab") { lanes.reopen(checkout) }
                            Button("Discard…", role: .destructive) { discarding = checkout }
                        }
                    }
                }
            }
            Section("Recent") {
                ForEach(Recents.paths.filter { $0 != model.repoPath }, id: \.self) { path in
                    Button((path as NSString).lastPathComponent) {
                        Task {
                            await model.open(project: path)
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
            Button(Flags.scaffolding ? "New Project…   ⇧⌘N" : "Clone from GitHub…   ⇧⌘N") {
                NotificationCenter.default.post(name: .keelNewProject, object: nil)
            }
            Divider()
            Button("Reveal in Finder") {
                NSWorkspace.shared.selectFile(nil, inFileViewerRootedAtPath: model.repoPath)
            }
        } label: {
            HStack(spacing: prominent ? 6 : 4) {
                Image(systemName: "folder.fill").font(K.F.ui(prominent ? 12 : 10))
                Text((model.repoPath as NSString).lastPathComponent)
                    .font(prominent ? K.F.body.weight(.semibold) : K.F.codeTiny.weight(.medium))
                Image(systemName: "chevron.down")
                    .font(K.F.ui(prominent ? 9 : 6, .bold))
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
        // Asks the daemon first, exactly as the rail's own discard does: it is the only thing that
        // knows what is on the branch and what was never committed at all.
        .alert("Discard this feature?", isPresented: Binding(get: { discarding != nil },
                                                             set: { if !$0 { discarding = nil } })) {
            Button("Discard", role: .destructive) {
                guard let checkout = discarding, let lanes = model.lanes else { return }
                Task {
                    if let why = await lanes.discardCheckout(checkout, force: false) {
                        refusal = why
                    }
                }
            }
            Button("Keep it", role: .cancel) {}
        } message: {
            Text(discarding.map { "\($0.branch) — its checkout and, if it is merged, its branch." }
                 ?? "")
        }
        .alert("Not discarded", isPresented: Binding(get: { refusal != nil },
                                                     set: { if !$0 { refusal = nil } })) {
            Button("Discard anyway", role: .destructive) {
                guard let checkout = discarding, let lanes = model.lanes else { return }
                Task { _ = await lanes.discardCheckout(checkout, force: true) }
            }
            Button("Keep it", role: .cancel) {}
        } message: {
            Text(refusal ?? "")
        }
    }

    /// One line per checkout: the branch, and what is on it that the project does not have.
    private static func describe(_ checkout: Wire.Worktree) -> String {
        var parts = [checkout.branch]
        if checkout.ahead > 0 { parts.append("\(checkout.ahead) ahead") }
        if checkout.dirty { parts.append("edited") }
        if checkout.ahead == 0 && !checkout.dirty { parts.append("nothing on it") }
        return parts.joined(separator: " · ")
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
    @Binding var terminalCommand: String?
    @Binding var paletteOpen: Bool
    @Binding var starting: Bool
    @Binding var panel: SessionWindow.Panel?
    @State private var confirmingTrust = false

    func body(content: Content) -> some View {
        content
            .modifier(ChatEvents(lanes: lanes, model: model, showSettings: $showSettings))
            .modifier(LaneEvents(lanes: lanes, panel: $panel))
            .onWindowCommand(.keelTrust) { _ in
                confirmingTrust = true
            }
            .modifier(TrustAlert(model: model, shown: $confirmingTrust))
            .onChange(of: lanes.waitingCount) { Notifications.badge(lanes.waitingCount) }
            .task(id: model.id) { await lanes.refreshShared() }
            .modifier(StageEvents(lanes: lanes, model: model, stage: $stage,
                                  showSettings: $showSettings))
            .onWindowCommand(.keelReviewTask) { _ in
                model.viewingDiff = nil
                model.viewingFile = nil
                model.viewingCommit = nil
                model.inspecting = nil
                withAnimation(K.M.quick) { stage = .review; showSettings = false }
            }
            .onWindowCommand(.keelPalette) { _ in
                withAnimation(K.M.quick) { paletteOpen.toggle() }
            }
            .onWindowCommand(.keelSettings) { _ in
                withAnimation(K.M.quick) { showSettings.toggle() }
            }
            .onWindowCommand(.keelNewLane) { _ in
                // The two questions a feature starts with — which project, from which branch —
                // instead of assuming the open one and wherever it is standing.
                NotificationCenter.default.post(name: .keelNewFeatureSheet, object: nil)
            }
            .onWindowCommand(.keelNewProject) { _ in
                starting = true
            }
            .onWindowCommand(.keelRunInTerminal) { note in
                // Open the terminal where you are — under Settings too, since that is where
                // the button lives — and hand it the command; it types it once the shell is up.
                terminalCommand = note.object as? String
                withAnimation(K.M.quick) { showTerminal = true }
            }
            .onWindowCommand(.keelToggleTerminal) { _ in
                withAnimation(K.M.quick) { showTerminal.toggle() }
            }
            .onWindowCommand(.keelOpenProject) { _ in
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
            await model.open(project: url.path)
            Recents.remember(url.path)
            await lanes.refreshShared()
        }
    }
}

/// ⌘⇧T. Trust is the highest-consequence switch in the app, so the keyboard path confirms and
/// names what it grants — the same words the approval card uses.
struct TrustAlert: ViewModifier {
    let model: SessionModel
    @Binding var shown: Bool
    struct TrustBody: Encodable { var trusted: Bool }

    /// What trusting grants, said once. There are two of these alerts — the status bar's and
    /// Settings' — and two copies of a promise is two promises.
    static let blurb = "The agent runs commands in this repository without asking. Stored in "
        + ".keel/permissions.json; withdrawable here or from the status bar."

    func body(content: Content) -> some View {
        content.alert("Trust this project?", isPresented: $shown) {
            Button("Trust") {
                Task {
                    do {
                        _ = try await model.client.post("/api/permissions/trust",
                                                        body: TrustBody(trusted: true),
                                                        as: Bool.self)
                    } catch {
                        // The alert dismisses either way. Without this the status bar simply
                        // stayed "not trusted" and there was nothing anywhere saying why.
                        model.lastError = "Could not trust this project: "
                            + error.localizedDescription
                    }
                    await model.refreshTrust()
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text(TrustAlert.blurb)
        }
    }
}

/// ⌘1–9, ⌘⇧] / ⌘⇧[, ⌘⇧E, and a click on a banner. Its own modifier because the window's
/// chain of handlers had grown past what the type-checker finishes in reasonable time.
private struct LaneEvents: ViewModifier {
    let lanes: Lanes
    @Binding var panel: SessionWindow.Panel?
    /// The last panel that was open, so ⌘⇧E brings back what you closed rather than a fixed one.
    @State private var lastPanel = SessionWindow.Panel.sessions

    func body(content: Content) -> some View {
        content
            .onWindowCommand(.keelFocusLane, addressed: true) { note in
                focus(note.object)
            }
            .onWindowCommand(.keelNextLane) { note in
                step((note.object as? Int) ?? 1)
            }
            .onWindowCommand(.keelTogglePanel) { _ in
                withAnimation(K.M.quick) {
                    if let open = panel {
                        lastPanel = open
                        panel = nil
                    } else {
                        panel = lastPanel
                    }
                }
            }
            .onWindowCommand(.keelShowPanel) { note in
                guard let raw = note.object as? String,
                      let p = SessionWindow.Panel(rawValue: raw),
                      SessionWindow.Panel.shown.contains(p) else { return }
                withAnimation(K.M.quick) { panel = p }
            }
    }

    /// Either a lane id (from a notification) or an index (from ⌘1–9).
    private func focus(_ object: Any?) {
        // What the rail draws. ⌘3 counting hidden lanes was off by one for every background
        // review, and could land on a lane with no tab anywhere to show it was selected.
        let all = lanes.shown
        if let n = object as? Int {
            if n < all.count { lanes.activeID = all[n].id }
        } else if let raw = object as? String, let id = UUID(uuidString: raw),
                  all.contains(where: { $0.id == id }) {
            lanes.activeID = id
        }
        NSApp.activate()
    }

    private func step(_ by: Int) {
        let all = lanes.shown
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
            .onWindowCommand(.keelSend) { _ in model.send() }
            .onWindowCommand(.keelStop) { _ in model.stop() }
            .onWindowCommand(.keelFocusComposer) { _ in
                showSettings = false
                model.focusComposerTick += 1
            }
            .onWindowCommand(.keelToggleMode) { _ in
                model.mode = model.mode == "plan" ? "acceptEdits" : "plan"
            }
            .onWindowCommand(.keelApprove, addressed: true) { note in
                let m = lane(named: note)
                if let p = m.pending.first { m.answer(p, allow: true, scope: "session") }
            }
            .onWindowCommand(.keelDeny, addressed: true) { note in
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
    /// The pane being sized is to the left of the handle (drag right = wider).
    var leading = false
    @State private var hovering = false
    @State private var start: Double?
    @State private var pushed = false

    var body: some View {
        Rectangle()
            .fill(hovering ? K.C.accent.opacity(0.6) : K.C.line)
            .frame(width: 1)
            .padding(.horizontal, K.S.xs)
            .contentShape(Rectangle())
            .onHover { over in
                hovering = over
                // Only pop what was pushed: a hover-out can arrive without a hover-in.
                if over, !pushed { NSCursor.resizeLeftRight.push(); pushed = true }
                if !over, pushed { NSCursor.pop(); pushed = false }
            }
            .gesture(
                // Global coordinates: the handle moves with the pane it sizes, so a
                // translation measured in its own space shifted under the pointer every
                // frame and the drag stuttered. Whole points, so the layout does not
                // re-solve for sub-pixel changes.
                DragGesture(minimumDistance: 1, coordinateSpace: .global)
                    .onChanged { g in
                        if start == nil { start = width }
                        // The stage is on the right, so dragging left makes it wider; a pane
                        // on the left is the other way round.
                        let delta = leading ? g.translation.width : -g.translation.width
                        let next = min(max((start ?? width) + delta, range.lowerBound), range.upperBound).rounded()
                        if next != width { var t = Transaction(); t.disablesAnimations = true; withTransaction(t) { width = next } }
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
            .onWindowCommand(.keelShowStage) { note in
                guard let raw = note.object as? String,
                      let s = SessionWindow.Stage(rawValue: raw),
                      SessionWindow.Stage.shown.contains(s) else { return }
                withAnimation(K.M.quick) {
                    stage = s
                    showSettings = false
                    model.viewingDiff = nil
                    model.viewingFile = nil
                    model.viewingCommit = nil
                    model.inspecting = nil
                }
            }
    }

    /// Something to look at is worth looking at — unless you are deliberately reading something
    /// else.
    private func showPreviewIfIdle() {
        guard Flags.designer, model.previewURL != nil, !showSettings,
              model.viewingDiff == nil, model.inspecting == nil,
              model.focusedTurn == nil else { return }
        withAnimation(K.M.quick) { stage = .preview }
    }

    /// The agent is writing the page. Show the page — that is the whole point of having one.
    private func followTheEdit() {
        guard Flags.designer, model.followEdits, model.editing != nil, model.previewURL != nil,
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
    ///
    /// And never off a diff, a file or a commit you opened on purpose. It used to clear all of
    /// them — plus `focusedTurn` — the instant a turn began, so reading a change and then asking a
    /// question about it threw the change away, and with no back stack there was no route to it.
    /// The stage follows the work when you are not already reading something.
    private func followTheWork() {
        guard model.running, model.id == lanes.activeID, stage != .preview,
              model.viewingDiff == nil, model.viewingFile == nil,
              model.viewingCommit == nil, model.inspecting == nil else { return }
        stage = .turn
        showSettings = false
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


/// The bar that narrates a project switch, then says what it found. A window that goes still
/// for three seconds and then pops a dialog reads as a hang; this reads as work.
struct OpeningBar: View {
    let name: String
    let stage: String
    let done: Bool

    var body: some View {
        HStack(spacing: K.S.sm) {
            if done {
                Image(systemName: "checkmark.circle.fill").font(K.F.micro).foregroundStyle(K.C.add)
            } else {
                ProgressView().controlSize(.small).frame(width: 12, height: 12)
            }
            Text(done ? "Opened \(name)" : "Opening \(name)")
                .font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
            Text(done ? stage : "\(stage)…").font(K.F.small).foregroundStyle(K.C.dim)
                .contentTransition(.opacity)
            Spacer()
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.snug)
        .background(done ? K.C.add.opacity(0.08) : K.C.surface)
        .overlay(alignment: .bottom) { Hairline() }
    }
}


/// The branch switcher behind the footer chip. Local branches first with the current one
/// marked and its distance from upstream, then the ones only the remote has; typing filters,
/// ⏎ switches to the first match, and a name nothing matches becomes "create it".
struct BranchMenu: View {
    let model: SessionModel
    let close: () -> Void
    @State private var query = ""
    @FocusState private var focused: Bool

    private var local: [Wire.Branch] {
        let all = model.branches?.local ?? []
        let q = query.trimmingCharacters(in: .whitespaces)
        return q.isEmpty ? all : all.filter { Fuzzy.score(q, in: $0.name) != nil }
    }
    private var remote: [String] {
        let names = Set((model.branches?.local ?? []).map(\.name))
        let all = (model.branches?.remote ?? []).filter { $0.contains("/") }
            .filter { r in !names.contains(String(r.split(separator: "/", maxSplits: 1).last ?? "")) }
        let q = query.trimmingCharacters(in: .whitespaces)
        return q.isEmpty ? all : all.filter { Fuzzy.score(q, in: $0) != nil }
    }
    private var canCreate: Bool {
        let q = query.trimmingCharacters(in: .whitespaces)
        return !q.isEmpty && !(model.branches?.local ?? []).contains { $0.name == q }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            TextField("Switch to or create a branch…", text: $query)
                .textFieldStyle(.plain).font(K.F.small)
                .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
                .focused($focused)
                .onSubmit {
                    if let first = local.first { switchTo(first.name) }
                    else if let r = remote.first { switchTo(r) }
                    else if canCreate { create() }
                }
            Hairline()
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    if !local.isEmpty { RailHeader("Local", trailing: "\(local.count)") }
                    ForEach(local, id: \.name) { b in
                        HoverRow(selected: b.current) {
                            HStack(spacing: K.S.sm) {
                                Image(systemName: b.current ? "checkmark" : "arrow.triangle.branch")
                                    .font(K.F.tiny).frame(width: 12)
                                    .foregroundStyle(b.current ? K.C.accent : K.C.faint)
                                Text(b.name).font(K.F.codeSmall).foregroundStyle(K.C.text).lineLimit(1)
                                Spacer()
                                if b.ahead > 0 { Text("\(b.ahead)↑").font(K.F.codeTiny).foregroundStyle(K.C.add) }
                                if b.behind > 0 { Text("\(b.behind)↓").font(K.F.codeTiny).foregroundStyle(K.C.accent) }
                                if b.upstream == nil, !b.current {
                                    Text("local only").font(K.F.micro).foregroundStyle(K.C.faint)
                                }
                            }
                        } action: { if !b.current { switchTo(b.name) } }
                    }
                    if !remote.isEmpty { RailHeader("On the remote only", trailing: "\(remote.count)") }
                    ForEach(remote, id: \.self) { r in
                        HoverRow {
                            HStack(spacing: K.S.sm) {
                                Image(systemName: "icloud").font(K.F.tiny).frame(width: 12)
                                    .foregroundStyle(K.C.faint)
                                Text(r).font(K.F.codeSmall).foregroundStyle(K.C.text).lineLimit(1)
                                Spacer()
                                Text("check out").font(K.F.micro).foregroundStyle(K.C.faint)
                            }
                        } action: { switchTo(r) }
                    }
                    if canCreate {
                        Hairline().padding(.vertical, K.S.xs)
                        HoverRow {
                            HStack(spacing: K.S.sm) {
                                Image(systemName: "plus").font(K.F.tiny).frame(width: 12)
                                    .foregroundStyle(K.C.accent)
                                Text("Create branch ").font(K.F.small).foregroundStyle(K.C.text)
                                + Text(query.trimmingCharacters(in: .whitespaces)).font(K.F.codeSmall)
                                    .foregroundStyle(K.C.accent)
                                Spacer()
                                Text("from \(model.branch ?? "HEAD")").font(K.F.micro).foregroundStyle(K.C.faint)
                            }
                        } action: { create() }
                    }
                    if local.isEmpty, remote.isEmpty, !canCreate {
                        EmptyState("No branches yet.")
                            .padding(K.S.md)
                    }
                }
                .padding(.vertical, K.S.xs)
            }
            .frame(maxHeight: 320)
        }
        .frame(width: 340)
        .background(K.C.surface)
        .onAppear { focused = true }
    }

    private func switchTo(_ name: String) {
        close()
        Task { await model.branch("switch", name) }
    }
    private func create() {
        let name = query.trimmingCharacters(in: .whitespaces)
        close()
        Task { await model.branch("create", name) }
    }
}

/// A menu key, a ⌘K item or a toolbar button means *this* window — but every one of them travels
/// as a `NotificationCenter` post, which has no idea which window that is. So every open window
/// acted on every one of them: one ⌘N put the "New feature" sheet on two screens at once, and
/// filling in one of them left the other standing.
///
/// `SessionWindow.pinned` used to stand in for "the main window" and the sheet was guarded on it,
/// but nothing has passed it since a detached window got its own `Lanes` — the guard was reading a
/// parameter no caller sets. Asking AppKit which window is key cannot go quietly stale that way.
///
/// `addressed: true` is for the commands that also arrive from a click on a system notification,
/// which names its lane by id: that one belongs to whichever window holds that lane, key or not.
extension View {
    func onWindowCommand(_ name: Notification.Name, addressed: Bool = false,
                         perform act: @escaping (Notification) -> Void) -> some View {
        modifier(WindowCommand(name: name, addressed: addressed, act: act))
    }

    /// Put this at the root of a window so the commands inside it know which window they are in.
    func windowCommands(_ here: WindowHere) -> some View {
        background(WindowFinder(here: here)).environment(here)
    }
}

/// Which `NSWindow` a view is in, and whether the person is typing into it.
///
/// Not `controlActiveState`: an attached sheet takes key away from its parent, so a window with
/// the "New feature" sheet up reads as merely active — and ⌘. would have stopped no turn while
/// the sheet a person opened by accident was on screen. That is the "every wait ends" rule losing
/// to a modal. The sheet is part of the window it is attached to, so it counts as being here.
@MainActor @Observable final class WindowHere {
    var window: NSWindow?

    var isKey: Bool {
        guard let mine = window, let key = NSApp.keyWindow else { return false }
        return key === mine || key.sheetParent === mine || mine.attachedSheet === key
    }
}

/// A zero-size `NSView` whose only job is to report the window it landed in. SwiftUI has no
/// environment value for the window itself, and every route to one goes through AppKit.
private struct WindowFinder: NSViewRepresentable {
    let here: WindowHere

    func makeNSView(context: Context) -> NSView { NSView(frame: .zero) }

    func updateNSView(_ view: NSView, context: Context) {
        // `view.window` is nil until the view is in the hierarchy, which is after this call.
        DispatchQueue.main.async { [here] in here.window = view.window }
    }
}

struct WindowCommand: ViewModifier {
    let name: Notification.Name
    let addressed: Bool
    let act: (Notification) -> Void
    @Environment(WindowHere.self) private var here: WindowHere?

    /// A lane id is a String; ⌘1–9 sends an Int and a menu key sends nothing.
    ///
    /// `key` is nil in a window that has not found itself yet — one frame at launch, and any
    /// preview or test that renders a pane on its own. Acting there is the old behaviour and the
    /// safe end of the trade: a command that runs twice beats ⌘↵ doing nothing on the first send.
    static func acts(key: Bool?, addressed: Bool, object: Any?) -> Bool {
        (key ?? true) || (addressed && object is String)
    }

    func body(content: Content) -> some View {
        content.onReceive(NotificationCenter.default.publisher(for: name)) { note in
            guard Self.acts(key: here?.isKey, addressed: addressed, object: note.object) else {
                return
            }
            act(note)
        }
    }
}
