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
    let onClose: () -> Void

    @State private var section: Section = .connections

    enum Section: String, CaseIterable, Identifiable {
        case connections, permissions, devices, appearance, privacy
        var id: String { rawValue }

        var title: String {
            switch self {
            case .connections: "Tools"
            case .permissions: "Permissions"
            case .devices: "Devices"
            case .appearance: "Appearance"
            case .privacy: "Privacy"
            }
        }
        var icon: String {
            switch self {
            case .connections: "wrench.and.screwdriver"
            case .permissions: "lock"
            case .devices: "iphone"
            case .appearance: "paintbrush"
            case .privacy: "hand.raised"
            }
        }
        var blurb: String {
            switch self {
            case .connections: "the CLIs Keel drives"
            case .permissions: "what the agent may run here"
            case .devices: "what can reach this Mac"
            case .appearance: "how Keel looks"
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
                        Text("Back to the project").font(K.F.small.weight(.semibold))
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

            ForEach(Section.allCases) { s in
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

    private var needsAttention: Int {
        model.tools.count { !$0.installed || !$0.authenticated }
    }

    private var detail: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: K.S.lg) {
                Text(section.title).font(K.F.display).foregroundStyle(K.C.text)

                switch section {
                case .connections: ConnectionsSettings(client: model.client)
                case .permissions: PermissionsSettings(client: model.client,
                                                       sessionId: model.sessionId)
                case .devices: PairingSettings(model: pairing)
                case .appearance: AppearanceSettings()
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
        VStack(alignment: .leading, spacing: K.S.md) {
            Text("System follows the Mac. Both palettes are complete and contrast-checked; dark "
                 + "is the one the diffs were designed in. The icon in the toolbar switches too.")
                .font(K.F.body).foregroundStyle(K.C.dim)
                .fixedSize(horizontal: false, vertical: true)

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


/// What leaves the machine, and the switches that stop it.
struct PrivacySettings: View {
    @State private var crashes = Telemetry.crashReports
    @State private var usage = Telemetry.usage
    @State private var sent = false

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            Text("Two things can leave this Mac, and nothing else: a crash report when Keel "
                 + "crashes, and counts of what was used — \"a turn finished, the checks passed, "
                 + "three files\". Never a prompt, a file, a path, or a repository name.")
                .font(K.F.body).foregroundStyle(K.C.dim)
                .fixedSize(horizontal: false, vertical: true)

            Toggle("Send crash reports", isOn: $crashes)
                .toggleStyle(.switch)
                .onChange(of: crashes) { Telemetry.crashReports = crashes }
                .disabled(!Telemetry.sentryConfigured)
            Toggle("Send anonymous usage", isOn: $usage)
                .toggleStyle(.switch)
                .onChange(of: usage) { Telemetry.usage = usage }
                .disabled(!Telemetry.posthogConfigured)

            if !Telemetry.sentryConfigured && !Telemetry.posthogConfigured {
                Text("This build has no reporting keys, so nothing is sent either way.")
                    .font(K.F.small).foregroundStyle(K.C.faint)
            } else if Telemetry.sentryConfigured {
                Button(sent ? "Sent — check the dashboard" : "Send a test report") {
                    Telemetry.sendTest(); sent = true
                }
                .buttonStyle(QuietButton())
                .disabled(!crashes)
            }

            Text("Version \(Telemetry.version)"
                 + (Updater.shared.available ? " · updates automatically" : ""))
                .font(K.F.micro).foregroundStyle(K.C.faint)
        }
    }
}
