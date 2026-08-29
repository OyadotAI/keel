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
        case connections, permissions, devices, appearance
        var id: String { rawValue }

        var title: String {
            switch self {
            case .connections: "Tools"
            case .permissions: "Permissions"
            case .devices: "Devices"
            case .appearance: "Appearance"
            }
        }
        var icon: String {
            switch self {
            case .connections: "wrench.and.screwdriver"
            case .permissions: "lock"
            case .devices: "iphone"
            case .appearance: "paintbrush"
            }
        }
        var blurb: String {
            switch self {
            case .connections: "the CLIs Keel drives"
            case .permissions: "what the agent may run here"
            case .devices: "what can reach this Mac"
            case .appearance: "how Keel looks"
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
                    .font(.system(size: 10, weight: .semibold)).tracking(0.7)
                    .foregroundStyle(K.C.faint)
                Spacer()
                CloseButton(size: 10) { onClose() }
            }
            .padding(.horizontal, K.S.md).padding(.top, K.S.md).padding(.bottom, K.S.sm)

            ForEach(Section.allCases) { s in
                HoverRow(selected: section == s) {
                    HStack(spacing: K.S.sm) {
                        Image(systemName: s.icon)
                            .font(.system(size: 11))
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
                                .font(.system(size: 10, weight: .bold))
                                .foregroundStyle(K.C.bg)
                                .padding(.horizontal, 3).padding(.vertical, 1)
                                .background(K.C.warn, in: Capsule())
                        }
                    }
                    .padding(.vertical, 3)
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
    @AppStorage("keel.appearance") private var appearance = "dark"

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            Text("Keel is a reading surface for code and diffs, so it is dark by default. The "
                 + "light palette is complete and contrast-checked; it is just not what this is "
                 + "for.")
                .font(K.F.body).foregroundStyle(K.C.dim)
                .fixedSize(horizontal: false, vertical: true)

            HStack(spacing: 0) {
                ForEach(["dark", "light"], id: \.self) { mode in
                    let on = appearance == mode
                    Text(mode.capitalized)
                        .font(K.F.small.weight(on ? .semibold : .regular))
                        .foregroundStyle(on ? K.C.text : K.C.faint)
                        .padding(.horizontal, K.S.md).padding(.vertical, 5)
                        .background(
                            RoundedRectangle(cornerRadius: K.R.sm - 1)
                                .fill(on ? K.C.raised : .clear).padding(1)
                        )
                        .contentShape(Rectangle())
                        .onTapGesture {
                            appearance = mode
                            NSApp.appearance = NSAppearance(
                                named: mode == "light" ? .aqua : .darkAqua)
                        }
                }
            }
            .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
        }
    }
}
