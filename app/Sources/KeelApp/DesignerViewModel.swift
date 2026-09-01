import AppKit
import Foundation
import SwiftUI

/// The Designer's state: the pins, the page, the bridge to the web view, and the dev server as
/// this lane sees it.
///
/// One object per lane, read through the lane's forwarding accessors so the views did not have
/// to change with the move. The intents that use this — picking, nudging, the pixel check,
/// following an edit to its page — stay on the lane for now, because they also start turns and
/// read the transcript; what moved is the state they write, which used to sit among a hundred
/// other things on the same class.
@MainActor
@Observable
final class DesignerViewModel {
    /// Elements picked in the preview, waiting to go with the next ask.
    var pins: [SessionModel.Pin] = []
    /// The pins the running turn was sent with, photographed again when it finishes.
    var designInFlight: [SessionModel.Pin] = []
    /// The page, lent by the pane that owns the web view. See `PreviewCanvas`.
    var resnapshot: ((Picked.Rect) async -> NSImage?)?
    var rectsNow: (([String]) async -> [String: Picked.Rect])?
    var canvas: (([String: Any]) -> Void)?
    /// Which pane installed the bridge, so an old pane's teardown does not take a new one's.
    weak var canvasOwner: AnyObject?
    /// The frontend file the agent is writing right now, when it is one.
    var editing: String?
    /// Whether the window goes to the page when the agent edits it. Off by default: the trace is
    /// the tab engineers read, and a stage that switches to the page on its own is the thing
    /// they turned off first. On is a choice, remembered.
    var followEdits: Bool = UserDefaults.standard.object(forKey: "keel.followEdits") as? Bool ?? false {
        didSet {
            UserDefaults.standard.set(followEdits, forKey: "keel.followEdits")
            designTick += 1
        }
    }
    /// Bumped when the window should show the preview because the agent is editing it.
    var designTick = 0
    /// What the last write changed on the page, as the page reported it.
    var changedRegions: [Region] = []
    var previewURL: String?
    var devRunning = false
    /// The lane whose dev server is running, when it is not this one's.
    var devElsewhere: String?
    var picking = false
    var detachedPreview = false
    var previewWidth: PreviewWidth = .desktop
    /// The dev server's last lines, for when the page is blank and the reason is in them.
    var devLog: [String] = []
    /// What went wrong loading the page, from the web view itself.
    var previewProblem: String?
    /// Bumped to ask the web view to reload the page it has.
    var reloadTick = 0
}

/// What the workbench is showing beside the conversation: a detour into a diff, a file, a
/// commit or an inspection; a sheet; the turn both panes are focused on; the prompts that are
/// mid-answer. View state that two panes or the window read, per lane — so it is neither a
/// pane's own `@State` nor a fact about the project.
@MainActor
@Observable
final class WorkbenchViewModel {
    /// The file whose diff is on screen, picked from the changes tree.
    var viewingDiff: String?
    /// The file whose contents are on screen, picked from the file tree.
    var viewingFile: String?
    var viewingCommit: Wire.Commit?
    var inspecting: SessionModel.Inspect?
    var sheet: SessionModel.Sheet?
    /// The turn the person clicked into, which both panes scroll to and the trace stops
    /// following for.
    var focusedTurn: UUID?
    var renamingSession: String?
    var renameDraft = ""
    var confirmingDiscard = false
}
