import AppKit
import UserNotifications

/// Telling you what happened when you are not looking.
///
/// This is the whole of what Orca's mobile companion is praised for — "kick off agents, walk away,
/// get pings when each finishes" — for the case that is actually 95% of it: you are in another
/// window on the same machine. That case needs no phone, no QR pairing and no relay, which is
/// three hundred issues of theirs that this simply does not have.
///
/// Nothing fires while Keel is frontmost. A notification for something you just watched happen is
/// noise, and noise is how people turn notifications off.
///
/// Every notification names its lane and carries its id: clicking one lands on the lane that
/// posted it, and three lanes finishing are three banners you can tell apart. The identifier is
/// the lane *and the kind*, so a second job replaces the first job rather than stacking — and
/// does not replace the approval you have not answered yet.
@MainActor
enum Notifications {
    // Everything here touches NSApp, which is main-actor anyway, so the whole enum lives there
    // rather than each member arguing about it.
    private static var authorized = false

    /// What macOS thinks of us, in the words Settings shows.
    ///
    /// It is read and kept because the alternative is the failure this app is least able to
    /// afford: `requestAuthorization` returns `granted: false` immediately and forever once
    /// somebody has said no — including somebody who said no to a build from three months ago —
    /// and every notification then does not happen, with a settings page full of switches that
    /// are all on and nothing anywhere saying why nothing arrives.
    enum Permission: Equatable {
        case unknown, allowed, denied, unasked
    }
    private(set) static var permission: Permission = .unknown

    /// What Keel can tell you about, and what a person can turn off one at a time.
    ///
    /// A kind rather than a scatter of booleans: each one owns its own default, its own throttle
    /// and its own line in Settings, so adding the next one is a case rather than an edit in four
    /// places — and so the switch that turns a kind off cannot be forgotten at the post site.
    enum Kind: String, CaseIterable, Identifiable {
        /// The turn ended: what it changed, and what the gate said.
        case turn
        /// A command is waiting to be approved, with the answer on the banner.
        case approval
        /// A background job started, and again when it finishes.
        case job
        /// The agent wrote a file.
        case file

        var id: String { rawValue }

        var title: String {
            switch self {
            case .turn: "When a turn finishes"
            case .approval: "When something needs approving"
            case .job: "When a background job starts and finishes"
            case .file: "When files are written"
            }
        }

        var note: String {
            switch self {
            case .turn: "What changed and what the checks said, so the banner answers rather than asking you to look."
            case .approval: "Approve and Deny are on the banner — you do not have to come back to the window."
            case .job: "A job outlives the turn that started it, so this is usually the one you walked away from."
            case .file: "One banner at a time, at most every 20 seconds, naming the newest file and the count so far."
            }
        }

        /// A turn writes twenty files and a banner each would be a firehose, so the file kind is
        /// coalesced rather than counted out. The others happen once and are worth hearing about
        /// the moment they do.
        var throttle: TimeInterval { self == .file ? 20 : 0 }

        /// Every kind is on out of the box. The ones that could be noisy are throttled instead of
        /// being shipped switched off, because a feature nobody discovers is one nobody has.
        var onByDefault: Bool { true }

        private var key: String { "keel.notify.\(rawValue)" }

        /// `bool(forKey:)` is `false` for a key that has never been written, which would ship every
        /// kind silently off. The default belongs to the kind, so the absence has to be asked about.
        var enabled: Bool {
            get {
                UserDefaults.standard.object(forKey: key) as? Bool ?? onByDefault
            }
            nonmutating set {
                UserDefaults.standard.set(newValue, forKey: key)
            }
        }
    }

    /// Whether a banner makes a sound. On, because a silent banner in the corner of a screen you
    /// are not looking at is a notification you did not get.
    static var sound: Bool {
        get { UserDefaults.standard.object(forKey: "keel.notify.sound") as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: "keel.notify.sound") }
    }

    static func prepare() {
        UNUserNotificationCenter.current()
            .requestAuthorization(options: [.alert, .sound, .badge]) { granted, _ in
                // The callback arrives off the main thread.
                Task { @MainActor in
                    authorized = granted
                    await refreshPermission()
                }
            }
        Task { await refreshPermission() }
        UNUserNotificationCenter.current().setNotificationCategories([
            UNNotificationCategory(
                identifier: approvalCategory,
                actions: [
                    UNNotificationAction(identifier: "allow", title: "Approve"),
                    UNNotificationAction(identifier: "deny", title: "Deny",
                                         options: [.destructive]),
                ],
                intentIdentifiers: [])
        ])
    }

    /// Ask macOS rather than remembering what it said. The answer changes outside the app — in
    /// System Settings, or with a Focus mode — and `requestAuthorization` is only asked once.
    static func refreshPermission() async {
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        permission = switch settings.authorizationStatus {
        case .authorized, .provisional, .ephemeral: .allowed
        case .denied: .denied
        case .notDetermined: .unasked
        @unknown default: .unknown
        }
        // The status is the authority, not the one-shot callback: a build authorised on a
        // previous launch never sees `granted` again.
        authorized = permission == .allowed
    }

    /// One banner, now, whatever else is true — the button in Settings.
    ///
    /// It ignores the frontmost rule on purpose. Every real notification is suppressed while Keel
    /// is the app in front, which is exactly where you are standing when you go looking for why
    /// you are not getting any, so a test that obeyed the rule would prove nothing and read as
    /// another failure.
    static func test() {
        let content = UNMutableNotificationContent()
        content.title = "Keel"
        content.body = "Notifications are working. This is what a banner looks like."
        if sound { content.sound = .default }
        UNUserNotificationCenter.current().add(
            UNNotificationRequest(identifier: "keel.test", content: content, trigger: nil))
    }

    static let approvalCategory = "keel.approval"

    private static var frontmost: Bool { NSApplication.shared.isActive }

    /// When each kind last posted, per lane. The throttle is per lane on purpose: three lanes
    /// writing files at once are three conversations, and coalescing them together would hide two.
    private static var lastPost: [String: Date] = [:]

    /// Whether enough time has passed for this kind to post again.
    ///
    /// Its own function so the rule can be asserted without a notification centre, a window, or a
    /// twenty-second wait in a test.
    static func due(_ kind: Kind, since last: Date?, now: Date = Date()) -> Bool {
        guard let last else { return true }
        return now.timeIntervalSince(last) >= kind.throttle
    }

    private static func post(_ kind: Kind, _ title: String, _ body: String, lane: SessionModel,
                             category: String? = nil) {
        guard authorized, !frontmost, kind.enabled else { return }
        let slot = "\(lane.id.uuidString)-\(kind.rawValue)"
        guard due(kind, since: lastPost[slot]) else { return }
        lastPost[slot] = Date()

        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.userInfo = ["lane": lane.id.uuidString]
        if sound { content.sound = .default }
        if let category { content.categoryIdentifier = category }
        UNUserNotificationCenter.current().add(
            UNNotificationRequest(identifier: slot, content: content, trigger: nil))
    }

    /// The body carries the verdict, so the notification answers rather than asking you to look.
    static func turnFinished(lane: SessionModel, files: Int, gate: Turn.Gate) {
        let verdict: String
        switch gate {
        case .passed(let cmd, _): verdict = "\(cmd) passed"
        case .failed(let cmd, let problems):
            verdict = problems.isEmpty ? "\(cmd) failed" : "\(cmd) failed — \(problems.count) problem\(problems.count == 1 ? "" : "s")"
        case .none, .notRun, .running: verdict = "no gate ran"
        }
        let changed = files == 1 ? "1 file changed" : "\(files) files changed"
        post(.turn, lane.title, "\(changed) · \(verdict)", lane: lane)
    }

    static func approvalWaiting(lane: SessionModel, _ count: Int) {
        post(.approval, lane.title,
             count == 1 ? "The agent needs a command approved."
                        : "\(count) commands need approving.",
             lane: lane, category: approvalCategory)
        badge(count)
    }

    /// Keel took a command off the turn and is running it itself. Said out loud because nobody is
    /// asked any more: the banner is where "something is now running that outlives this turn"
    /// gets seen, and the Monitors panel is where it gets stopped.
    static func jobStarted(lane: SessionModel, command: String) {
        post(.job, lane.title, "Watching \(short(command))", lane: lane)
    }

    /// The one you actually walked away for. A job outlives its turn by design, so this is often
    /// the only thing that will tell you the CI run you asked about eight minutes ago is done.
    static func jobFinished(lane: SessionModel, command: String, exit: Int?) {
        let verdict = switch exit {
        case 0?: "finished"
        case let code?: "failed — exit \(code)"
        case nil: "stopped"
        }
        post(.job, lane.title, "\(short(command)) \(verdict)", lane: lane)
    }

    /// The agent wrote something. Throttled and replacing, so a turn that writes thirty files is
    /// one banner that keeps up rather than thirty that bury each other.
    static func fileWritten(lane: SessionModel, path: String, count: Int) {
        let name = path.split(separator: "/").last.map(String.init) ?? path
        let so_far = count <= 1 ? "" : " · \(count) files so far"
        post(.file, lane.title, "Wrote \(name)\(so_far)", lane: lane)
    }

    /// A command as a banner can hold it. The whole thing is in the panel; this is the label.
    private static func short(_ command: String) -> String {
        let line = command.split(separator: "\n").first.map(String.init) ?? command
        return line.count <= 60 ? line : String(line.prefix(59)) + "…"
    }

    /// Pending approvals across every window, on the Dock icon.
    static func badge(_ count: Int) {
        NSApp.dockTile.badgeLabel = count > 0 ? String(count) : nil
    }

    /// What a click or an action on a banner does. Routed through the same notifications the
    /// menu bar uses, so the window answers the way it answers ⌘⇧A.
    static func handle(lane raw: String?, action: String) {
        guard let raw else { return }
        switch action {
        case "allow":
            NotificationCenter.default.post(name: .keelApprove, object: raw)
        case "deny":
            NotificationCenter.default.post(name: .keelDeny, object: raw)
        default:
            NotificationCenter.default.post(name: .keelFocusLane, object: raw)
        }
    }
}
