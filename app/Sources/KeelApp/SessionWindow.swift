import AppKit
import SwiftUI

/// One window, one conversation.
///
/// A single command band, navigation, conversation and an inset inspector — laid out by hand
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
    @State private var inspectorVisible: Bool
    private let remembersPresentation: Bool
    @State private var expandedInspector = false
    @State private var compactSidebar = false
    @State private var lastSidebarPanel: Panel = .sessions
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var statusOpen = false
    @State private var statusTrust = false
    @State private var showTerminal = false
    @State private var terminalTitle = "shell"
    @State private var terminalCommand: String?
    @State private var paletteOpen = false
    @State private var starting = false
    @State private var startingFeature = false
    @State private var showSettings = false
    /// How wide the record beside the conversation is. Yours to drag; remembered.
    @AppStorage("keel.stageWidth") private var stageWidth: Double = 420
    /// And the side panel.
    @AppStorage("keel.panelWidth") private var panelWidth: Double = 320

    init(lanes: Lanes, pairing: PairingModel, app: AppModel? = nil,
         inspectorOpen: Bool? = nil, inspectorExpanded: Bool = false) {
        _lanes = State(initialValue: lanes)
        self.pairing = pairing
        self.app = app
        remembersPresentation = inspectorOpen == nil
        _inspectorVisible = State(initialValue: inspectorOpen ?? UserDefaults.standard.bool(forKey: "keel.inspectorVisible"))
        _expandedInspector = State(initialValue: inspectorExpanded)
    }

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
            case .sessions: "Sessions"
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
            if (lanes.atStart || !model.projectOpen) && showSettings {
                VStack(spacing: 0) {
                    SettingsPage(model: model, pairing: pairing, atWelcome: true) {
                        showSettings = false
                        showTerminal = false
                    }
                    if showTerminal {
                        Hairline()
                        terminalPane
                    }
                }
            } else if lanes.atStart {
                Welcome(model: model, daemonFailure: app?.failure,
                        onSettings: { showSettings = true }) { lanes.atStart = false }
            } else if model.projectOpen {
                workbench
            } else {
                Welcome(model: model, daemonFailure: app?.failure,
                        onSettings: { showSettings = true }) { }
            }
        }
        // The animations these three values drive are the opening bar's, and they live on the bar
        // in `workbench`. On this root they animated the whole window — every pane, the transcript
        // and the terminal — for the length of a `quick`, on a flag that swaps a "Reading…"
        // placeholder. `model.loaded` is dropped rather than moved: nothing transitions on it.
        .background(K.C.bg)
        // The toolbar draws no background of its own: the tab strip's title-bar material runs up
        // under it, so the title and the tabs are one surface (`LaneTabs`).
        .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        // Workbench commands are mounted only with a project. Machine setup must also work
        // before one exists, including installers that need the interactive terminal.
        .onWindowCommand(.keelSettings) { _ in
            guard lanes.atStart || !model.projectOpen else { return }
            showSettings.toggle()
            if !showSettings { showTerminal = false }
        }
        .onWindowCommand(.keelRunInTerminal) { note in
            guard lanes.atStart || !model.projectOpen else { return }
            terminalCommand = note.object as? String
            showTerminal = true
        }
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
            model.refreshCheckoutSoon()
        }
    }

    private var workbench: some View {
        VStack(spacing: 0) {
            // The lanes are always on screen, across the top. They are the reason this is a
            // window and not a terminal: several agents working at once, and you can see all
            // of them. Directly under the title bar, because the two share one material.
            workspaceHeader
            if let app, !app.crashes.isEmpty {
                CrashBar(reports: app.crashes) { Crashes.markSeen(); app.crashes = [] }
                Hairline()
            }
            // Its own stack, so the animation that moves it is the bar's and not the window's.
            VStack(spacing: 0) {
                if let o = model.opening {
                    OpeningBar(name: o.name, stage: o.stage, done: false)
                        .transition(.move(edge: .top).combined(with: .opacity))
                } else if let name = model.justOpened {
                    OpeningBar(name: name, stage: "\(model.changes.count) changed · \(model.findings.count) findings", done: true)
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
            }
            .animation(K.M.quick, value: model.opening)
            .animation(K.M.quick, value: model.justOpened)
            HStack(spacing: 0) {
                if layout.sidebar > 0 {
                    workspaceSidebar
                        .frame(width: layout.sidebar)
                    SplitHandle(width: $panelWidth, range: 300...420, reset: 320, leading: true)
                }
                if showSettings {
                    SettingsPage(model: model, pairing: pairing) {
                        showSettings = false
                        model.focusComposerTick += 1
                    }
                    .frame(maxWidth: .infinity)
                } else {
                    working
                }
            }
            .overlay(alignment: .leading) {
                if compactSidebar && layout.sidebar == 0 && panel != nil {
                    ZStack(alignment: .leading) {
                        Color.black.opacity(0.12)
                            .onTapGesture { compactSidebar = false }
                            .accessibilityHidden(true)
                        workspaceSidebar.frame(width: min(360, available - 40))
                            .background(K.C.surface)
                            .shadow(color: K.C.shadow, radius: 20, x: 8, y: 0)
                    }
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
        }
        .overlay(alignment: .top) { paletteOverlay }
        .modifier(TrustAlert(model: model, shown: $statusTrust))
        .toolbar { toolbar }
        .navigationTitle((model.repoPath as NSString).lastPathComponent.isEmpty
                         ? "Keel" : (model.repoPath as NSString).lastPathComponent)
        .navigationSubtitle(model.branch ?? "")
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
                SkillCatalog(client: model.client, autoCommit: model.autoCommit) {
                    model.sheet = nil
                    Task { await model.refreshState(); await model.refreshSuggestions() }
                }
            case .newSkill:
                NewSkill(client: model.client, autoCommit: model.autoCommit) {
                    model.sheet = nil
                    Task { await model.refreshState() }
                }
            case .subagent:
                NewSubagent(client: model.client, autoCommit: model.autoCommit) {
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
        .onChange(of: model.id) {
            traceUnread = false
            expandedInspector = false
            if model.workbench.detour != nil { inspectorVisible = true }
        }
        .onChange(of: model.workbench.inspectorRequest) {
            if model.workbench.detour != nil { inspectorVisible = true; compactSidebar = false }
        }
        .onChange(of: inspectorVisible) {
            if remembersPresentation { UserDefaults.standard.set(inspectorVisible, forKey: "keel.inspectorVisible") }
        }
        .onChange(of: panel) {
            if let panel { lastSidebarPanel = panel }
            compactSidebar = panel != nil && layout.sidebar == 0
            if panel == nil { model.focusComposerTick += 1 }
        }
        .transaction { if reduceMotion { $0.animation = nil } }
        .modifier(WindowEvents(
            lanes: lanes, model: model,
            stage: $stage, showSettings: $showSettings, showTerminal: $showTerminal, terminalCommand: $terminalCommand,
            paletteOpen: $paletteOpen, starting: $starting, panel: $panel,
            inspectorVisible: $inspectorVisible, expandedInspector: $expandedInspector,
            onToggleSidebar: toggleSidebar))
        .onWindowCommand(.keelShowPanel) { note in
            guard let raw = note.object as? String, let target = Panel(rawValue: raw),
                  Panel.shown.contains(target) else { return }
            showSettings = false
            compactSidebar = true
        }
        .onWindowCommand(.keelShowStage) { _ in compactSidebar = false }
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

    @State private var available: Double = 0

    private var layout: WorkspaceLayout {
        WorkspaceLayout.resolve(width: available == 0 ? 1100 : available,
                                sidebar: panelWidth, inspector: stageWidth,
                                wantsSidebar: panel != nil && !showSettings,
                                wantsInspector: inspectorVisible && !showSettings,
                                expanded: expandedInspector)
    }

    private var workspaceSidebar: some View {
        VStack(spacing: 0) {
            ProjectMenu(model: model, prominent: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(K.S.lg)
            ActivityRail(panel: $panel, model: model, onSettings: {
                showSettings.toggle()
                compactSidebar = false
            }, onPanel: { showSettings = false })
            if let panel {
                SidePanel(panel: panel, model: model)
            }
        }
        .spatialChrome()
    }

    private var workspaceHeader: some View {
        HStack(spacing: K.S.sm) {
            Button(action: toggleSidebar) { Image(systemName: "sidebar.left") }
                .buttonStyle(WorkspaceButton(selected: layout.sidebar > 0 || compactSidebar))
                .hint("Toggle sidebar (⌘⇧E)")
            LaneTabs(lanes: lanes, embedded: true, showsProject: layout.sidebar == 0 && available >= 900)
                .frame(maxWidth: .infinity)
            Button { statusOpen.toggle() } label: {
                Image(systemName: model.project.events.connected || model.project.port == 0
                      ? "waveform.path" : "wifi.slash")
                    .foregroundStyle(model.project.events.connected || model.project.port == 0 ? K.C.accent : K.C.warn)
            }
            .buttonStyle(WorkspaceButton(selected: statusOpen))
            .hint("Connection, branch and session usage")
            .popover(isPresented: $statusOpen, arrowEdge: .bottom) {
                WorkspaceStatus(model: model, onTrust: {
                    statusOpen = false
                    statusTrust = true
                }, onSettings: {
                    statusOpen = false
                    showSettings = true
                })
            }
            Button { showTerminal.toggle() } label: { Image(systemName: "terminal") }
                .buttonStyle(WorkspaceButton(selected: showTerminal))
                .hint("Toggle terminal (⌘⌥T)")
            if layout.inspectorOnly {
                Button { closeInspector() } label: {
                    Label("Conversation", systemImage: "arrow.left")
                }.buttonStyle(WorkspaceButton())
            } else {
                Button { openInspector(.review) } label: {
                    Image(systemName: "square.on.square")
                }.buttonStyle(WorkspaceButton(selected: inspectorVisible && stage == .review))
                    .hint("Review changes")
            }
            Button {
                if inspectorVisible { closeInspector() } else { openInspector(.turn) }
            } label: { Image(systemName: "sidebar.right") }
                .buttonStyle(WorkspaceButton(selected: inspectorVisible))
                .hint(inspectorVisible ? "Close inspector (⌘⌥I)" : "Open activity inspector (⌘⌥I)")
        }
        .padding(.horizontal, K.S.md)
        .frame(height: 52)
        .spatialChrome()
        .overlay(alignment: .bottom) { Hairline() }
    }

    private func toggleSidebar() {
        if layout.sidebar > 0 || compactSidebar {
            panel = nil
            compactSidebar = false
        } else {
            panel = lastSidebarPanel
            compactSidebar = true
        }
    }

    private func openInspector(_ target: Stage) {
        stage = target
        closeDetour()
        inspectorVisible = true
        showSettings = false
        compactSidebar = false
    }

    private func closeInspector() {
        inspectorVisible = false
        expandedInspector = false
        model.focusComposerTick += 1
    }

    /// Keep the conversation alive behind full-width review so draft, selection and scroll
    /// ownership survive. Only its visibility and hit testing change.
    private var working: some View {
        ZStack(alignment: .trailing) {
            ChatRail(model: model)
                .id(model.id)
                .padding(.trailing, inspectorVisible && !layout.inspectorOnly ? layout.inspector + 9 : 0)
                .opacity(layout.inspectorOnly ? 0 : 1)
                .allowsHitTesting(!layout.inspectorOnly)
                .accessibilityHidden(layout.inspectorOnly)
            if inspectorVisible {
                HStack(spacing: 0) {
                    if !layout.inspectorOnly {
                        SplitHandle(width: $stageWidth, range: 320...900, reset: 420)
                    }
                    inspectorSurface
                        .padding(.vertical, K.S.md)
                        .padding(.trailing, K.S.md)
                        .padding(.leading, layout.inspectorOnly ? K.S.md : 0)
                        .frame(width: layout.inspectorOnly ? nil : layout.inspector)
                }
                .frame(maxWidth: layout.inspectorOnly ? .infinity : nil)
                .frame(width: layout.inspectorOnly ? nil : layout.inspector + 9)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .clipped()
        .animation(reduceMotion ? nil : K.M.settle, value: inspectorVisible)
        .animation(reduceMotion ? nil : K.M.settle, value: expandedInspector)
    }

    private var inspectorSurface: some View {
        VStack(spacing: 0) {
            stageBar
            detourBar
            Group {
                if case .skill(let skill) = model.inspecting {
                    SkillPane(model: model, skill: skill)
                } else if let target = model.inspecting {
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
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .floatingSurface()
    }

    private var stageBar: some View {
        HStack(spacing: K.S.xxs) {
            ForEach(Stage.shown, id: \.self) { target in
                Button {
                    stage = target
                    closeDetour()
                } label: {
                    HStack(spacing: K.S.snug) {
                        Image(systemName: stageIcon(target))
                        Text(target.rawValue)
                        if target == .turn && traceUnread && stage != .turn {
                            Circle().fill(K.C.accent).frame(width: 5, height: 5)
                        }
                    }
                }
                .buttonStyle(WorkspaceButton(selected: stage == target))
                .accessibilityAddTraits(stage == target ? .isSelected : [])
            }
            Spacer(minLength: 0)
            if available >= 900 {
                Button { expandedInspector.toggle() } label: {
                    Image(systemName: expandedInspector ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right")
                }
                .buttonStyle(WorkspaceButton())
                .hint(expandedInspector ? "Restore split view" : "Expand inspector")
            }
            CloseButton(label: "Back to conversation", action: closeInspector)
        }
        .padding(K.S.sm)
        .spatialChrome()
        .overlay(alignment: .bottom) { Hairline() }
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
        model.workbench.detour = nil
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

    private var terminalPane: some View {
        VStack(spacing: 0) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "terminal").font(K.F.tiny).foregroundStyle(K.C.faint)
                Text(terminalTitle).font(K.F.codeTiny).foregroundStyle(K.C.dim)
                Spacer()
                CloseButton(size: 10, label: "Close the terminal") {
                    withAnimation(K.M.quick) { showTerminal = false }
                    model.focusComposerTick += 1
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
        // Terminal and session details live beside the tabs in the workspace command band.
    }
}

// MARK: - Activity rail

/// Labeled destinations keep the everyday workspace discoverable; configuration lives in one menu.
struct ActivityRail: View {
    @Binding var panel: SessionWindow.Panel?
    let model: SessionModel
    var onSettings: () -> Void = {}
    var onPanel: () -> Void = {}

    private let primary: [SessionWindow.Panel] = [.sessions, .changes, .files, .git]

    var body: some View {
        VStack(spacing: K.S.sm) {
            HStack(spacing: K.S.xxs) {
                ForEach(primary) { item in
                    Button { panel = item; onPanel() } label: {
                        VStack(spacing: K.S.xs) {
                            Image(systemName: item.icon).font(K.F.small)
                            Text(item.title).font(K.F.micro)
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, K.S.xs)
                    }
                    .buttonStyle(WorkspaceButton(selected: panel == item))
                    .hint(help(item))
                    .accessibilityAddTraits(panel == item ? .isSelected : [])
                }
            }
            HStack(spacing: K.S.sm) {
                Menu {
                    ForEach(SessionWindow.Panel.shown.filter { !primary.contains($0) }) { item in
                        Button { panel = item; onPanel() } label: {
                            Label(item.title, systemImage: item.icon)
                        }
                    }
                } label: {
                    Label(panel.map { primary.contains($0) ? "Tools & agents" : $0.title } ?? "Tools & agents",
                          systemImage: "slider.horizontal.3")
                        .font(K.F.small).foregroundStyle(K.C.dim)
                }
                .menuStyle(.borderlessButton)
                Spacer(minLength: 0)
                Button(action: onSettings) { Image(systemName: "gearshape") }
                    .buttonStyle(WorkspaceButton())
                    .hint(toolsNeedAttention > 0 ? "Settings — tools need attention (⌘,)" : "Settings (⌘,)")
            }
        }
        .padding(K.S.sm)
        .spatialChrome()
        .overlay(alignment: .bottom) { Hairline() }
    }

    /// The tooltip carries the reason for the badge, so a number on an icon is not a riddle.
    private func help(_ p: SessionWindow.Panel) -> String {
        let n = model.missingSuggestions.count
        switch p {
        case .plugins where n > 0:
            return "Plugins — \(n) recommended for this repository, not installed"
        case .skills where n > 0:
            return "Skills — \(n) recommended plugin\(n == 1 ? "" : "s") would add skills here"
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
        // What this repository is missing. On Skills as well as Plugins: the skills are what is
        // missing, and people look for them under Skills — with no number there, a repository
        // short of its recommended skills looked complete. Both panels list them with Install.
        case .plugins, .skills: model.missingSuggestions.count.nonZero
        default: nil
        }
    }
}

private extension Int {
    var nonZero: Int? { self == 0 ? nil : self }
}

// MARK: - Status bar

/// The instrument readout. Everything here answers a question you would otherwise have to go and
/// ask: which branch, is this project trusted, what will the gate run, what has this cost.
/// Session details stay available without taking a permanent row from the conversation.
struct WorkspaceStatus: View {
    let model: SessionModel
    var onTrust: () -> Void = {}
    var onSettings: () -> Void = {}
    @State private var branchMenu = false

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.lg) {
            Label(model.project.events.connected || model.project.port == 0 ? "Workspace status" : "Reconnecting to Keel",
                  systemImage: "waveform.path")
                .font(K.F.title).foregroundStyle(K.C.text)
            if model.isWorkspace {
                ForEach(model.repos) { repo in
                    detail(repo.name, repo.branch ?? "No branch")
                }
            } else if model.isRepo {
                Button {
                    Task { await model.refreshBranches() }
                    branchMenu = true
                } label: {
                    Label(model.branch ?? "No branch", systemImage: "arrow.triangle.branch")
                        .lineLimit(1).truncationMode(.middle)
                }
                .buttonStyle(WorkspaceButton())
                .popover(isPresented: $branchMenu) {
                    BranchMenu(model: model) { branchMenu = false }
                }
            } else {
                detail("Git", "Not initialized")
            }
            detail("Permissions", model.trusted ? "Trusted project" : "Approval required")
            detail("Checks", model.gateCommand ?? "Not configured")
            detail("Changed files", "\(model.changes.count)")
            if let context = model.contextTokens { detail("Context", compact(context) + " tokens") }
            if let tokens = model.sessionTokens { detail("Session", compact(tokens.total) + " tokens") }
            if let cost = model.sessionCost { detail("Cost", money(cost)) }
            if model.running, let rate = model.burnRate { detail("Spend rate", money(rate) + "/min") }
            HStack(spacing: K.S.sm) {
                if !model.trusted {
                    Button("Project permissions…", action: onTrust).buttonStyle(QuietButton())
                }
                Button("Settings…", action: onSettings).buttonStyle(QuietButton())
            }
        }
        .padding(K.S.xl)
        .frame(width: 360)
        .background(K.C.surface)
    }

    private func detail(_ title: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: K.S.lg) {
            Text(title).font(K.F.small).foregroundStyle(K.C.dim)
            Spacer(minLength: 0)
            Text(value).font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                .multilineTextAlignment(.trailing).textSelection(.enabled)
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
                    .lineLimit(1).truncationMode(.middle)
                    .frame(maxWidth: prominent ? 160 : 220)
                Image(systemName: "chevron.down")
                    .font(K.F.ui(prominent ? 9 : 6, .bold))
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
    @Binding var inspectorVisible: Bool
    @Binding var expandedInspector: Bool
    var onToggleSidebar: (() -> Void)? = nil
    @State private var confirmingTrust = false

    func body(content: Content) -> some View {
        content
            .modifier(ChatEvents(lanes: lanes, model: model, showSettings: $showSettings))
            .modifier(LaneEvents(lanes: lanes, panel: $panel, onToggleSidebar: onToggleSidebar))
            .onWindowCommand(.keelTrust) { _ in
                confirmingTrust = true
            }
            .modifier(TrustAlert(model: model, shown: $confirmingTrust))
            .onChange(of: lanes.waitingCount) { Notifications.badge(lanes.waitingCount) }
            .task(id: model.id) {
                // A lane switch is not a project open: the project was read when it was opened.
                // What a lane has of its own is its checkout, so that is what is re-read here.
                // The full nine-call pass on every ⌘-tab is what made switching lanes feel slow.
                // Only a checkout nobody has read yet: one store per checkout, and events keep a
                // read one current, so re-reading on every switch was requests for nothing.
                guard model.repoPath.isEmpty else {
                    if model.repo.treeVersion == 0 { model.refreshCheckoutSoon() }
                    return
                }
                await lanes.refreshShared()
            }
            .modifier(StageEvents(lanes: lanes, model: model, stage: $stage,
                                  showSettings: $showSettings, inspectorVisible: $inspectorVisible))
            .onWindowCommand(.keelReviewTask) { _ in
                model.workbench.detour = nil
                withAnimation(K.M.quick) { stage = .review; showSettings = false; inspectorVisible = true }
            }
            .onWindowCommand(.keelToggleInspector) { _ in
                inspectorVisible.toggle()
                showSettings = false
                if !inspectorVisible {
                    expandedInspector = false
                    model.focusComposerTick += 1
                }
            }
            .onWindowCommand(.keelExpandInspector) { _ in
                inspectorVisible = true
                expandedInspector.toggle()
                showSettings = false
            }
            .onWindowCommand(.keelFocusComposer) { _ in
                expandedInspector = false
                inspectorVisible = false
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
    var onToggleSidebar: (() -> Void)? = nil
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
                if let onToggleSidebar { onToggleSidebar(); return }
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

    /// The lane a notification named, or the focused one if no id was given (e.g. keyboard shortcut).
    /// If an explicit id was given but does not belong to this window's lanes, returns nil.
    private func lane(named note: Notification) -> SessionModel? {
        if let raw = note.object as? String, let id = UUID(uuidString: raw) {
            return lanes.lanes.first(where: { $0.id == id })
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
                guard let m = lane(named: note), let p = m.pending.first else { return }
                guard !p.isQuestion, !p.isPlan else {
                    showSettings = false
                    m.focusComposerTick += 1
                    return
                }
                m.answer(p, allow: true, scope: "once")
            }
            .onWindowCommand(.keelDeny, addressed: true) { note in
                guard let m = lane(named: note), let p = m.pending.first else { return }
                m.answer(p, allow: false, scope: "session")
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
    @Binding var inspectorVisible: Bool

    func body(content: Content) -> some View {
        content
            .onChange(of: model.previewURL) { showPreviewIfIdle() }
            .onChange(of: model.designer.wantsStage) {
                guard model.designer.wantsStage else { return }
                model.designer.wantsStage = false
                followTheEdit()
            }
            .onChange(of: model.running) { followTheWork() }
            .onChange(of: model.workbench.traceRequest) { showTrace() }
            .onWindowCommand(.keelShowStage) { note in
                guard let raw = note.object as? String,
                      let s = SessionWindow.Stage(rawValue: raw),
                      SessionWindow.Stage.shown.contains(s) else { return }
                withAnimation(K.M.quick) {
                    stage = s
                    inspectorVisible = true
                    showSettings = false
                    model.workbench.detour = nil
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
            inspectorVisible = true
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
        inspectorVisible = true
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

    func makeNSView(context: Context) -> NSView {
        let view = FindingView()
        view.here = here
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {
        if let f = view as? FindingView { f.here = here }
        if let w = view.window { here.window = w }
    }
}

private final class FindingView: NSView {
    weak var here: WindowHere?
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if let window { here?.window = window }
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
