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
            // Lanes are the point of the window, so they get the number keys.
            CommandMenu("Lanes") {
                ForEach(1...9, id: \.self) { n in
                    Button("Lane \(n)") {
                        NotificationCenter.default.post(name: .keelFocusLane, object: n - 1)
                    }
                    .keyboardShortcut(KeyEquivalent(Character("\(n)")), modifiers: .command)
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
        if UserDefaults.standard.object(forKey: "keel.appearance") == nil {
            UserDefaults.standard.set("dark", forKey: "keel.appearance")
        }
        NSApp.appearance = UserDefaults.standard.string(forKey: "keel.appearance") == "light"
            ? NSAppearance(named: .aqua) : NSAppearance(named: .darkAqua)
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
        Button("New Session") {
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

    /// Flipped once the daemon is up, so the delegate can be handed a reference to shut down.
    var ready = false

    func shutdown() {
        bonjour.stop()
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
}
