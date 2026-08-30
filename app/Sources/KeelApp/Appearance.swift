import AppKit
import SwiftUI

/// Dark, light, or whatever the Mac is doing. One place applies it, so the toolbar menu and
/// the Settings control cannot disagree.
enum Appearance: String, CaseIterable, Identifiable {
    case system, dark, light
    var id: String { rawValue }

    static let key = "keel.appearance"

    static var current: Appearance {
        get { Appearance(rawValue: UserDefaults.standard.string(forKey: key) ?? "") ?? .system }
        set { UserDefaults.standard.set(newValue.rawValue, forKey: key); newValue.apply() }
    }

    func apply() {
        switch self {
        case .system: NSApp.appearance = nil
        case .dark: NSApp.appearance = NSAppearance(named: .darkAqua)
        case .light: NSApp.appearance = NSAppearance(named: .aqua)
        }
    }

    var title: String {
        switch self {
        case .system: "System"
        case .dark: "Dark"
        case .light: "Light"
        }
    }

    var icon: String {
        switch self {
        case .system: "circle.lefthalf.filled"
        case .dark: "moon"
        case .light: "sun.max"
        }
    }
}

/// The toolbar menu: the current mode's icon, the three choices under it.
struct AppearanceMenu: View {
    @State private var mode = Appearance.current

    var body: some View {
        Menu {
            ForEach(Appearance.allCases) { a in
                Button {
                    mode = a
                    Appearance.current = a
                } label: {
                    Label(a.title, systemImage: a.icon)
                    if a == mode { Image(systemName: "checkmark") }
                }
            }
        } label: {
            Image(systemName: mode.icon)
        }
        .hint("Appearance: \(mode.title)")
    }
}
