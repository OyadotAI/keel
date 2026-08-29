import AppKit
import UserNotifications

/// Telling you a turn is done when you are not looking.
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
/// finished, and three lanes finishing are three banners you can tell apart. One identifier per
/// lane, so a second event replaces the first rather than stacking.
@MainActor
enum Notifications {
    // Everything here touches NSApp, which is main-actor anyway, so the whole enum lives there
    // rather than each member arguing about it.
    private static var authorized = false

    static func prepare() {
        UNUserNotificationCenter.current()
            .requestAuthorization(options: [.alert, .sound, .badge]) { granted, _ in
                // The callback arrives off the main thread.
                Task { @MainActor in authorized = granted }
            }
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

    static let approvalCategory = "keel.approval"

    private static var frontmost: Bool { NSApplication.shared.isActive }

    private static func post(_ title: String, _ body: String, lane: SessionModel,
                             category: String? = nil) {
        guard authorized, !frontmost else { return }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.userInfo = ["lane": lane.id.uuidString]
        if let category { content.categoryIdentifier = category }
        UNUserNotificationCenter.current().add(
            UNNotificationRequest(identifier: lane.id.uuidString, content: content, trigger: nil))
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
        post(lane.title, "\(changed) · \(verdict)", lane: lane)
    }

    static func approvalWaiting(lane: SessionModel, _ count: Int) {
        post(lane.title,
             count == 1 ? "The agent needs a command approved."
                        : "\(count) commands need approving.",
             lane: lane, category: approvalCategory)
        badge(count)
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
