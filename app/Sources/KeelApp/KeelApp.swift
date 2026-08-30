import SwiftUI
import UserNotifications

@main
struct KeelApp: App {
    @State private var app = AppModel()
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        // `Window`, not `WindowGroup`. A group is duplicable, and macOS turns duplicates into
        // native tabs — so ⌘N produced three tabs all rendering the same shared lanes, which is
        // exactly the "three tabs, one session" it looked like. There is one window here by
        // design; concurrency lives in the lanes inside it.
        Window("Keel", id: "main") {
            SessionWindow(lanes: app.lanes, pairing: app.pairing, app: app)
                .task {
                    delegate.app = app
                    await app.start()
                }
                // The panes inside add up to 1062 before any of them gets its ideal width.
                .frame(minWidth: 1080, minHeight: 620)
                // The window draws its own chrome: a translucent background would put the desktop
                // behind a diff, and a dense reading surface needs an opaque ground.
                .containerBackground(K.C.bg, for: .window)
        }
        .windowToolbarStyle(.unifiedCompact(showsTitle: true))
        .defaultSize(width: 1440, height: 900)

        // A feature torn out of the tab strip. One window per lane id, sharing the same lanes
        // and the same daemon — two agents working where you can watch both, which is what
        // dragging a tab out of a browser does and what people expect here.
        // A feature pulled out of the strip. Its own daemon, its own project, its own tabs —
        // two windows used to be two views of one daemon, which is why they mirrored each other.
        WindowGroup(id: "feature", for: Detached.self) { $request in
            if let request {
                DetachedWindow(app: app, request: request)
                    .frame(minWidth: 900, minHeight: 560)
                    .containerBackground(K.C.bg, for: .window)
            }
        }
        // Never restored across launches: the lane it names is gone by then, and what came back
        // was an empty window with nothing in it.
        .restorationBehavior(.disabled)
        .defaultSize(width: 1180, height: 820)
        .commands {
            // Replaced so ⌘N is a lane. Left alone, SwiftUI's New Window is a second copy of the
            // same window.
            CommandGroup(after: .appInfo) {
                Button("Check for Updates…") { Updater.shared.check() }
                    .disabled(!Updater.shared.available)
            }
            CommandGroup(replacing: .newItem) {
                NewSessionCommand()
                Button("New Project…") {
                    NotificationCenter.default.post(name: .keelNewProject, object: nil)
                }
                .keyboardShortcut("n", modifiers: [.command, .shift])
                Button("Open Project…") {
                    NotificationCenter.default.post(name: .keelOpenProject, object: nil)
                }
                .keyboardShortcut("o", modifiers: .command)
            }
            // The claim this app makes against a terminal is that someone who came from one never
            // has to reach for the mouse. That is a checklist, not an aspiration, so the bindings
            // live in the menu bar where they are discoverable rather than only in the views.
            CommandMenu("Agent") {
                Button("Send") { NotificationCenter.default.post(name: .keelSend, object: nil) }
                    .keyboardShortcut(.return, modifiers: .command)
                Button("Stop") { NotificationCenter.default.post(name: .keelStop, object: nil) }
                    .keyboardShortcut(".", modifiers: .command)
                Divider()
                Button("Focus composer") {
                    NotificationCenter.default.post(name: .keelFocusComposer, object: nil)
                }
                .keyboardShortcut("l", modifiers: .command)
                Button("Toggle plan mode") {
                    NotificationCenter.default.post(name: .keelToggleMode, object: nil)
                }
                .keyboardShortcut(.tab, modifiers: .shift)
                Divider()
                Button("Terminal") {
                    NotificationCenter.default.post(name: .keelToggleTerminal, object: nil)
                }
                .keyboardShortcut("t", modifiers: [.command, .option])
                Button("Settings") {
                    NotificationCenter.default.post(name: .keelSettings, object: nil)
                }
                .keyboardShortcut(",", modifiers: .command)
                Button("Command Palette") {
                    NotificationCenter.default.post(name: .keelPalette, object: nil)
                }
                .keyboardShortcut("k", modifiers: .command)
                Divider()
                Button("Approve oldest request") {
                    NotificationCenter.default.post(name: .keelApprove, object: nil)
                }
                .keyboardShortcut("a", modifiers: [.command, .shift])
                Button("Deny oldest request") {
                    NotificationCenter.default.post(name: .keelDeny, object: nil)
                }
                .keyboardShortcut("d", modifiers: [.command, .shift])
                Button("Trust this project…") {
                    NotificationCenter.default.post(name: .keelTrust, object: nil)
                }
                .keyboardShortcut("t", modifiers: [.command, .shift])
            }
            // Lanes are the point of the window, so they get the number keys — the lanes that
            // exist, by name. Nine fixed "Lane n" entries was a menu of things that did nothing.
            CommandMenu("Lanes") {
                ForEach(Array(app.lanes.lanes.prefix(9).enumerated()), id: \.element.id) { i, lane in
                    Button(lane.title == "Untitled" ? "Feature \(i + 1) (empty)" : lane.title) {
                        NotificationCenter.default.post(name: .keelFocusLane, object: i)
                    }
                    .keyboardShortcut(KeyEquivalent(Character("\(i + 1)")), modifiers: .command)
                }
                Divider()
                Button("Next lane") {
                    NotificationCenter.default.post(name: .keelNextLane, object: 1)
                }
                .keyboardShortcut("]", modifiers: [.command, .shift])
                Button("Previous lane") {
                    NotificationCenter.default.post(name: .keelNextLane, object: -1)
                }
                .keyboardShortcut("[", modifiers: [.command, .shift])
                Divider()
                Button("Toggle side panel") {
                    NotificationCenter.default.post(name: .keelTogglePanel, object: nil)
                }
                .keyboardShortcut("e", modifiers: [.command, .shift])
            }
        }
    }
}

/// Shutting the daemon down when the app goes away.
///
/// SwiftUI has no scene-level "we are quitting" hook that runs before the process exits, so this is
/// an `NSApplicationDelegate`. Without it the daemon is simply orphaned: it is spawned as a child,
/// nothing signals it, and macOS has no `PR_SET_PDEATHSIG` for the child to notice with — so it
/// reparents to init and keeps serving. That is precisely the accumulation Orca is bug-reported
/// for, and it was happening here until a launch/quit cycle was actually watched.
final class AppDelegate: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    @MainActor var app: AppModel?

    @MainActor
    func applicationWillTerminate(_ notification: Notification) {
        Telemetry.flush()
        app?.shutdown()
    }

    /// A banner's Approve/Deny buttons were registered and then never listened for, so they did
    /// nothing; clicking the banner did nothing either. This is the listener.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter,
                                            didReceive response: UNNotificationResponse) async {
        let lane = response.notification.request.content.userInfo["lane"] as? String
        let action = response.actionIdentifier
        await MainActor.run { Notifications.handle(lane: lane, action: action) }
    }

    /// Dark by default. Keel is a reading surface for code and diffs, and the palette is built
    /// dark-first — light exists and is complete, but it is not what this is for.
    func applicationDidFinishLaunching(_ notification: Notification) {
        UNUserNotificationCenter.current().delegate = self
        // The Mac's own setting unless someone chose otherwise. Dark-by-default was the
        // design's preference; the person's is the one that counts.
        Appearance.current.apply()
    }

    /// Closing the window quits, because a Keel with no window is a daemon with a Dock icon.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    /// No native tabs. There is one window, and stacking copies of it into a tab bar makes the
    /// lanes inside look duplicated when they are not.
    func applicationWillFinishLaunching(_ notification: Notification) {
        NSWindow.allowsAutomaticWindowTabbing = false
    }
}

/// A lane, not a window: the point is seeing them together.
private struct NewSessionCommand: View {
    var body: some View {
        Button("New Feature…") {
            NotificationCenter.default.post(name: .keelNewLane, object: nil)
        }
        .keyboardShortcut("n", modifiers: .command)
    }
}

/// What every window shares: one daemon, one client, and one model per session.
@MainActor
@Observable
final class AppModel {
    private let daemon: Daemon
    private let client: Client
    /// Handed to settings panes, which talk to the daemon without owning a session.
    var sharedClient: Client { client }
    var failure: String?
    private var started = false
    let pairing: PairingModel

    init(port: UInt16 = 7777) {
        daemon = Daemon(port: port)
        let c = Client(port: port)
        client = c
        pairing = PairingModel(client: c)
        lanes = Lanes(client: c, port: port)
    }

    private let bonjour = Bonjour()

    /// The windows that were pulled out of the strip, each with a daemon of its own.
    private var detached: [UUID: Workspace] = [:]

    /// The workspace for a torn-out window, started on first ask and kept until it closes.
    func workspace(for request: Detached) -> Workspace {
        if let existing = detached[request.lane] { return existing }
        // Created synchronously so the window has something to draw; the daemon comes up under it.
        let placeholder = Workspace(port: 0)
        detached[request.lane] = placeholder
        Task { @MainActor in
            let port = await Workspace.freePort()
            let real = Workspace(port: port)
            detached[request.lane] = real
            await real.start(project: request.project, resume: request.session)
        }
        return placeholder
    }

    func closeWorkspace(_ id: UUID) {
        detached.removeValue(forKey: id)?.shutdown()
    }

    /// Flipped once the daemon is up, so the delegate can be handed a reference to shut down.
    var ready = false

    func shutdown() {
        bonjour.stop()
        for (_, w) in detached { w.shutdown() }
        detached.removeAll()
        daemon.stop()
    }

    func start() async {
        guard !started else { return }
        started = true
        Telemetry.start()
        Telemetry.track("app_opened")
        Updater.shared.start()
        crashes = Crashes.unseen()
        Notifications.prepare()
        do { try await daemon.start() } catch { failure = error.localizedDescription }
        ready = true
        await advertiseIfReachable()
    }

    /// Advertise only when the daemon actually answers off this machine.
    ///
    /// Asked of the daemon rather than assumed from a setting: the bind is decided in `serve.rs`,
    /// which downgrades to loopback when nothing is paired or Tailscale is down. Reading the
    /// setting here would advertise a machine that quietly refused to listen.
    private func advertiseIfReachable() async {
        struct Devices: Decodable {}
        let devices: [PairingModel.Device] = (try? await client.get("/api/pair/devices")) ?? []
        guard !devices.isEmpty else { return }
        let state: Wire.State? = try? await client.get("/api/state")
        let repo = (state?.repo as NSString?)?.lastPathComponent ?? "a project"
        bonjour.advertise(port: daemon.port, repo: repo)
    }

    /// Reports macOS wrote for crashes since the last launch that looked.
    var crashes: [Crashes.Report] = []

    /// One workspace of lanes, shared by the window. Sessions used to be one model per window,
    /// which is why several could not be seen at once.
    let lanes: Lanes
}


/// Menu commands cannot reach a window's model directly, so they are announced and the focused
/// window answers. Cheaper than threading a focused-value binding through every view for five
/// shortcuts.
extension Notification.Name {
    static let keelSend = Notification.Name("keel.send")
    static let keelStop = Notification.Name("keel.stop")
    static let keelFocusComposer = Notification.Name("keel.focusComposer")
    static let keelToggleMode = Notification.Name("keel.toggleMode")
    static let keelApprove = Notification.Name("keel.approve")
    static let keelNewFeatureSheet = Notification.Name("keel.newFeatureSheet")
    static let keelNewLane = Notification.Name("keel.newLane")
    static let keelPalette = Notification.Name("keel.palette")
    static let keelSettings = Notification.Name("keel.settings")
    static let keelOpenProject = Notification.Name("keel.openProject")
    static let keelNewProject = Notification.Name("keel.newProject")
    static let keelToggleTerminal = Notification.Name("keel.toggleTerminal")
    static let keelDeny = Notification.Name("keel.deny")
    static let keelTrust = Notification.Name("keel.trust")
    /// Object: a lane id string, or an `Int` index from ⌘1–9.
    static let keelFocusLane = Notification.Name("keel.focusLane")
    /// Object: +1 or −1.
    static let keelNextLane = Notification.Name("keel.nextLane")
    static let keelTogglePanel = Notification.Name("keel.togglePanel")
    /// Object: the command to type into the terminal, opening it first.
    static let keelRunInTerminal = Notification.Name("keel.runInTerminal")
}


/// A feature in a window of its own, with a daemon of its own.
///
/// Nothing is shared with the window it came from except the transcript on disk, which is what
/// lets the conversation continue here: the workspace opens the same repository and resumes the
/// same session. From then on the two windows are independent — different projects, different
/// features, different agents running at once.
private struct DetachedWindow: View {
    let app: AppModel
    let request: Detached
    @Environment(\.dismissWindow) private var dismiss

    var body: some View {
        let workspace = app.workspace(for: request)
        Group {
            if workspace.ready {
                SessionWindow(lanes: workspace.lanes, pairing: workspace.pairing, app: app)
            } else if let failure = workspace.failure {
                message("This window could not start its own Keel.", detail: failure)
            } else {
                VStack(spacing: K.S.sm) {
                    ProgressView().controlSize(.small)
                    Text("Opening \(request.title) in its own window…")
                        .font(K.F.small).foregroundStyle(K.C.dim)
                    Text("It gets a Keel of its own, so this window can hold a different project.")
                        .font(K.F.micro).foregroundStyle(K.C.faint)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(K.C.bg)
            }
        }
        .navigationTitle(request.title)
        .onDisappear { app.closeWorkspace(request.lane) }
    }

    private func message(_ text: String, detail: String) -> some View {
        VStack(spacing: K.S.sm) {
            Image(systemName: "exclamationmark.triangle").font(.system(size: 20))
                .foregroundStyle(K.C.warn)
            Text(text).font(K.F.body).foregroundStyle(K.C.text)
            Text(detail).font(K.F.micro).foregroundStyle(K.C.faint)
                .multilineTextAlignment(.center).frame(maxWidth: 420)
            Button("Close this window") { dismiss() }.buttonStyle(QuietButton())
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(K.C.bg)
    }
}
