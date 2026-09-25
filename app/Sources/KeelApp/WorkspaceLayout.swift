import SwiftUI

/// Presentation geometry only. Resizing never overwrites the user's preferred widths.
struct WorkspaceLayout: Equatable {
    var sidebar: Double
    var inspector: Double
    var inspectorOnly: Bool

    static func resolve(width: Double, sidebar: Double = 320, inspector: Double = 420,
                        wantsSidebar: Bool, wantsInspector: Bool, expanded: Bool = false) -> Self {
        let width = max(0, width)
        let full = wantsInspector && (expanded || width < 900)
        let sidebarFloor = 1280.0
        let left = wantsSidebar && !full && width >= sidebarFloor
            ? min(max(300, sidebar), min(420, width - (wantsInspector ? 720 : 420))) : 0
        let right = wantsInspector
            ? (full ? width : min(max(320, inspector), width - left - 409)) : 0
        return Self(sidebar: left, inspector: max(0, right), inspectorOnly: full)
    }
}

