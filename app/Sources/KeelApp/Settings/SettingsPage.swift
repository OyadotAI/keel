import AppKit
import SwiftUI

/// Settings, as a page in the window.
///
/// It was a `Settings` scene: a floating panel with a tab strip, sized by AppKit and styled by
/// nobody. Everything in it is about the project you have open — which tools work, what the agent
/// may run here, which devices can reach it — so putting it in a window of its own meant leaving
/// the thing it describes.
///
/// Same shape as the rest of the app: a list on the left, the detail beside it.
struct SettingsPage: View {
    let model: SessionModel
    let pairing: PairingModel
    var atWelcome = false
    let onClose: () -> Void

    @State private var section: Section = .connections

    enum Section: String, CaseIterable, Identifiable {
        case connections, permissions, devices, appearance, notifications, privacy
        var id: String { rawValue }

        var title: String {
            switch self {
            case .connections: "Tools"
            case .permissions: "Permissions"
            case .devices: "Devices"
            case .appearance: "Appearance"
            case .notifications: "Notifications"
            case .privacy: "Privacy"
            }
        }
        var icon: String {
            switch self {
            case .connections: "wrench.and.screwdriver"
            case .permissions: "lock"
            case .devices: "iphone"
            case .appearance: "paintbrush"
            case .notifications: "bell"
            case .privacy: "hand.raised"
            }
        }
        var blurb: String {
            switch self {
            case .connections: "the CLIs Keel drives"
            case .permissions: "what the agent may run here"
            case .devices: "what can reach this Mac"
            case .appearance: "how Keel looks"
            case .notifications: "when Keel pings you"
            case .privacy: "crash reports and usage"
            }
        }
    }

    var body: some View {
        HStack(spacing: 0) {
            nav
            Rectangle().fill(K.C.line).frame(width: 1)
            detail
        }
        .background(K.C.bg)
    }

    private var nav: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("SETTINGS")
                    .sectionLabel()
                    .foregroundStyle(K.C.faint)
                Spacer()
                CloseButton(size: 10, label: "Close settings") { onClose() }
            }
            .padding(.horizontal, K.S.md).padding(.top, K.S.md).padding(.bottom, K.S.sm)

            // The way back, first and named. A close button in a corner is a thing you have to
            // already know about.
            HoverRow {
                HStack(spacing: K.S.sm) {
                    Image(systemName: "house").font(K.F.micro)
                        .foregroundStyle(K.C.accent).frame(width: 16)
                    VStack(alignment: .leading, spacing: 0) {
                        Text(atWelcome ? "Back to setup" : "Back to the project").font(K.F.small.weight(.semibold))
                            .foregroundStyle(K.C.text)
                        Text("or press Esc").font(K.F.micro).foregroundStyle(K.C.faint)
                    }
                    Spacer()
                }
                .padding(.vertical, K.S.tight)
            } action: {
                onClose()
            }
            .keyboardShortcut(.escape, modifiers: [])
            Hairline().padding(.vertical, K.S.xs)

            ForEach(Section.allCases.filter { !atWelcome || ($0 != .permissions && $0 != .devices) }) { s in
                HoverRow(selected: section == s) {
                    HStack(spacing: K.S.sm) {
                        Image(systemName: s.icon)
                            .font(K.F.micro)
                            .foregroundStyle(section == s ? K.C.text : K.C.faint)
                            .frame(width: 16)
                        VStack(alignment: .leading, spacing: 0) {
                            Text(s.title)
                                .font(K.F.small.weight(section == s ? .semibold : .regular))
                                .foregroundStyle(K.C.text)
                            Text(s.blurb).font(K.F.micro).foregroundStyle(K.C.faint)
                        }
                        Spacer()
                        if s == .connections, needsAttention > 0 {
                            Text("\(needsAttention)")
                                .font(K.F.tiny.weight(.bold))
                                .foregroundStyle(K.C.bg)
                                .padding(.horizontal, K.S.tight).padding(.vertical, K.S.hair)
                                .background(K.C.warn, in: Capsule())
                        }
                    }
                    .padding(.vertical, K.S.tight)
                } action: {
                    section = s
                }
            }
            Spacer()
        }
        .frame(width: 210)
        .background(K.C.surface)
    }

    var needsAttention: Int {
        model.tools.count { !$0.installed || !$0.authenticated }
    }

    private var detail: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: K.S.lg) {
                Text(section.title).font(K.F.display).foregroundStyle(K.C.text)

                switch section {
                case .connections: ConnectionsSettings(client: model.client, tools: model.tools,
                                                       onToolsChanged: { model.tools = $0 })
                case .permissions: PermissionsSettings(client: model.client,
                                                       sessionId: model.sessionId)
                case .devices: PairingSettings(model: pairing)
                case .appearance: AppearanceSettings()
                case .notifications: NotificationSettings()
                case .privacy: PrivacySettings()
                }
            }
            .frame(maxWidth: 640, alignment: .leading)
            .padding(K.S.xl)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

/// The one thing in here that is about Keel rather than about the project.
struct AppearanceSettings: View {
    @State private var mode = Appearance.current

    var body: some View {
        SettingsSection(
            "Appearance",
            note: "System follows the Mac. Both palettes are complete and contrast-checked; dark "
                + "is the one the diffs were designed in. The icon in the toolbar switches too."
        ) {
            HStack(spacing: 0) {
                ForEach(Appearance.allCases) { a in
                    let on = mode == a
                    Label(a.title, systemImage: a.icon)
                        .font(K.F.small.weight(on ? .semibold : .regular))
                        .foregroundStyle(on ? K.C.text : K.C.faint)
                        .padding(.horizontal, K.S.md).padding(.vertical, K.S.snug)
                        .background(
                            RoundedRectangle(cornerRadius: K.R.sm - 1)
                                .fill(on ? K.C.raised : .clear).padding(K.S.hair)
                        )
                        .contentShape(Rectangle())
                        .asButton { mode = a; Appearance.current = a }
                }
            }
            .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
        }
    }
}


/// When Keel pings you, one kind at a time.
///
/// Every kind is on out of the box and each one can be turned off on its own, because the thing
/// that makes notifications useful and the thing that makes them intolerable are the same
/// mechanism pointed at different events — and which is which is a matter of taste rather than
/// something Keel can work out.
struct NotificationSettings: View {
    /// Toggles read `UserDefaults` through `Notifications.Kind`; this is what makes the view
    /// redraw when one changes.
    @State private var tick = 0
    @State private var sound = Notifications.sound

    @State private var permission = Notifications.permission
    @State private var sent = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // First, because every switch below it is meaningless while macOS is saying no — and
            // "all my switches are on and nothing arrives" is a bug report about Keel.
            if permission != .allowed {
                SettingsSection(
                    "macOS is not letting these through",
                    note: permission == .denied
                        ? "Notifications for Keel are turned off in System Settings, so nothing "
                            + "below can post. macOS only asks once, and it remembers a no."
                        : "macOS has not been asked yet, or has not answered. Send a test and it "
                            + "will ask."
                ) {
                    Button("Open System Settings › Notifications") {
                        let url = "x-apple.systempreferences:com.apple.preference.notifications"
                        if let u = URL(string: url) { NSWorkspace.shared.open(u) }
                    }
                    .buttonStyle(QuietButton())
                }
            }

            SettingsSection(
                "What Keel tells you about",
                note: "Nothing is posted while Keel is the app in front — a banner for something "
                    + "you just watched happen is noise. Clicking one opens the lane it came from."
            ) {
                ForEach(Notifications.Kind.allCases) { kind in
                    SettingsToggle(kind.title, note: kind.note, isOn: Binding(
                        get: { _ = tick; return kind.enabled },
                        set: { kind.enabled = $0; tick += 1 }))
                }
            }

            SettingsSection(
                "Sound",
                note: "A silent banner in the corner of a screen you are not looking at is a "
                    + "notification you did not get."
            ) {
                SettingsToggle("Play a sound", isOn: $sound)
                    .onChange(of: sound) { Notifications.sound = sound }

                // The same affordance the crash reporter has, for the same reason: the only way
                // to tell "Keel is not posting" from "macOS is not showing" is to post one on
                // purpose and watch for it.
                Button(sent ? "Sent — look for the banner" : "Send a test notification") {
                    Notifications.test()
                    sent = true
                    Task { await Notifications.refreshPermission(); permission = Notifications.permission }
                }
                .buttonStyle(QuietButton())
                .padding(.top, K.S.xs)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .task {
            await Notifications.refreshPermission()
            permission = Notifications.permission
        }
    }
}


/// What leaves the machine, and the switches that stop it.
struct PrivacySettings: View {
    @State private var crashes = Telemetry.crashReports
    @State private var usage = Telemetry.usage
    @State private var sent = false

    /// One paragraph rather than two, so the general explanation does not end up printed *under*
    /// the specific one and reading backwards.
    private var privacyNote: String {
        let what = "Native app crash diagnostics go to Sentry; usage events go to PostHog. "
            + "Usage can include a persistent installation identifier. These switches control "
            + "future reporting, not deletion of reports already sent."
        guard Telemetry.sentryConfigured || Telemetry.posthogConfigured else {
            return what + " This build has no reporting keys, so nothing is sent either way."
        }
        return what
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSection("What leaves this Mac", note: privacyNote) {
                SettingsToggle("Send crash reports", isOn: $crashes)
                    .onChange(of: crashes) { Telemetry.crashReports = crashes }
                    .disabled(!Telemetry.sentryConfigured)
                SettingsToggle("Send usage events", isOn: $usage)
                    .onChange(of: usage) { Telemetry.usage = usage }
                    .disabled(!Telemetry.posthogConfigured)

                Link("Privacy details", destination: URL(string:
                    "https://github.com/OyadotAI/keel/blob/main/docs/privacy.md")!)
                    .font(K.F.small)

                if Telemetry.sentryConfigured {
                    Button(sent ? "Sent — check the dashboard" : "Send a test report") {
                        Telemetry.sendTest(); sent = true
                    }
                    .buttonStyle(QuietButton())
                    .disabled(!crashes)
                    .padding(.top, K.S.xs)
                }
            }

            SettingsSection("This build") {
                SettingsRow(title: "Version \(Telemetry.version)",
                            detail: Updater.shared.available
                                ? "Updates automatically." : "No update feed in this build.") {
                    EmptyView()
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
