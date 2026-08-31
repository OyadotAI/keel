import Foundation

/// The parts of Keel that are built but not shipped yet.
///
/// One place rather than a `UserDefaults` read at each call site, so what is currently hidden is
/// answerable by reading one file. Each is off by default and turned on with a `defaults write`,
/// which is enough for us and for a tester we are talking to — the point is that nobody finds them
/// by accident, not that they are unreachable.
///
/// Read once, at launch: a flag that changes under a running window would leave the stage on a tab
/// that no longer exists.
enum Flags {
    /// Scaffolding a new project from a template. There is one way into a project — open a folder
    /// or clone one — until this earns a second.
    ///
    /// `defaults write ai.oya.keel keel.newProject -bool YES`
    static let scaffolding = UserDefaults.standard.bool(forKey: "keel.newProject")

    /// The Designer: the preview stage, the element picker, the pixel column on a turn, and the
    /// redirects that bring the page forward while the agent writes it.
    ///
    /// `defaults write ai.oya.keel keel.designer -bool YES`
    static let designer = UserDefaults.standard.bool(forKey: "keel.designer")

    /// The readiness report: the scanner's findings panel, the "fix the blocking findings"
    /// suggestion, and the staff-engineer review that runs itself once a day. The scan itself
    /// comes free with `/api/state`, so this hides the surface rather than saving any work — the
    /// review is the one part that would otherwise spend a turn nobody can read.
    ///
    /// `defaults write ai.oya.keel keel.readiness -bool YES`
    static let readiness = UserDefaults.standard.bool(forKey: "keel.readiness")
}
