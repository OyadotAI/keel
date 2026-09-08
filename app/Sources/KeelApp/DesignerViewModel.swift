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
            wantsStage = followEdits
        }
    }
    /// The window should show the preview, because the agent is editing it and following is
    /// on. The window reads it and puts it back.
    var wantsStage = false
    /// What the last write changed on the page, as the page reported it.
    var changedRegions: [Region] = []
    /// Climbs with every `changed` report; `checkDesign` waits on it instead of on a timer.
    private var changeTick = 0
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
    /// Asks the page to load again, lent by the pane with the rest of the bridge.
    var reloadPage: (() -> Void)?

    /// Load the page again. The button used to re-poll the daemon and never touch the page.
    func reload() {
        previewProblem = nil
        reloadPage?()
    }

    /// Draw what the model knows onto the page: the pins.
    func syncCanvas() {
        canvas?(["keel": "pins", "pins": pins.map { ["id": $0.id.uuidString, "selector": $0.picked.selector] }])
    }

    func removePin(_ id: UUID) {
        pins.removeAll { $0.id == id }
        syncCanvas()
    }

    func clearRegions() {
        changedRegions = []
        // What you dragged was a way of saying it, not the change itself. Put the page back, so
        // what you are looking at when the turn lands is the agent's work and not your ghost.
        canvas?(["keel": "revert"])
        canvas?(["keel": "clear"])
        syncCanvas()
    }

    /// The page reported what moved. `turn` is the one the write belongs to — the current turn
    /// while one runs, the last one after — handed in by the lane, which owns the turns.
    func regionsChanged(_ regions: [Region], into turn: Turn?) {
        changedRegions = regions
        changeTick += 1
        if let turn, let union = Picked.Rect.union(regions.map(\.rect)) {
            Task {
                let shot = await resnapshot?(union) ?? nil
                var design = turn.design ?? Turn.Design()
                design.regions = regions
                design.pageAfter = shot
                turn.design = design
            }
        }
    }

    /// A change made by hand in the page: it lands on the pin for that element, making one if
    /// there is none, so dragging a handle is a complete instruction on its own.
    func designNudge(_ p: Picked, label: String) {
        if let i = pins.firstIndex(where: { $0.picked.selector == p.selector }) {
            pins[i].picked = p
            if !pins[i].nudges.contains(label) { pins[i].nudges.append(label) }
        } else {
            pins.append(SessionModel.Pin(picked: p, before: nil, nudges: [label]))
            Telemetry.track("nudge", [:])
        }
        syncCanvas()
    }

    /// The pins and what you typed, as the prompt that gets sent. One prompt for all of them:
    /// sending notes one at a time makes the agent swing back and forth.
    func designPrompt(_ instruction: String) -> String? {
        guard !pins.isEmpty else { return nil }
        var out = pins.count == 1
            ? "Change this element in the running app:\n\n"
            : "Change these \(pins.count) elements in the running app. Each is pinned on the page:\n\n"
        for (i, pin) in pins.enumerated() {
            if pins.count > 1 { out += "## Pin \(i + 1)\n" }
            out += pin.picked.describe()
            if !pin.nudges.isEmpty {
                out += "\n\nI changed this by hand in the running page, as a way of showing you "
                    + "what I want. Make the source produce this — do not add inline styles:\n"
                for n in pin.nudges { out += "  - \(n)\n" }
            }
            if !pin.note.isEmpty { out += "\n\nNote on this element: \(pin.note)" }
            out += "\n\n"
        }
        return out + instruction
    }

    /// Photograph the pinned elements again and say what the pixels did.
    ///
    /// Runs when the agent says `done`, before the daemon commits: "the tests broke and it edited
    /// the wrong file" is two facts, not one, and "it edited the wrong file" is a reason not to
    /// commit. Waits for the page to report a change rather than for a timer: the observer is the
    /// "HMR has landed" signal, and a fixed delay was wrong in both directions.
    func checkDesign(_ turn: Turn) async {
        let flight = designInFlight
        designInFlight = []
        guard !flight.isEmpty else { return }

        let seen = changeTick
        for _ in 0..<60 where changeTick == seen && !turn.files.isEmpty {
            try? await Task.sleep(for: .milliseconds(100))
        }
        if changeTick == seen { try? await Task.sleep(for: .milliseconds(400)) }

        var design = turn.design ?? Turn.Design()
        design.duplicated = DesignCheck.looksDuplicated(
            files: turn.files, hints: flight.flatMap(\.picked.hints))
        design.regions = design.regions.isEmpty ? changedRegions : design.regions

        guard let resnapshot else {
            design.pins = flight.map {
                .init(selector: $0.picked.selector, before: $0.before, after: nil,
                      verdict: .notCompared("the Designer was closed, so there was nothing to "
                                            + "photograph"))
            }
            turn.design = design
            return
        }

        // Where the elements are *now*. A rect captured at pick time is a square of the viewport,
        // and anything that scrolled between the click and here would have had the after-shot
        // taken of whatever moved into that square.
        let fresh = await rectsNow?(flight.map(\.picked.selector)) ?? [:]

        var checked: [Turn.Design.Pin] = []
        for pin in flight {
            let selector = pin.picked.selector
            // `rectsNow` is nil only when the pane went away between the guard above and here;
            // an empty answer from a page that did reply means nobody could resolve it.
            guard let rect = fresh[selector] ?? (rectsNow == nil ? pin.picked.rect : nil) else {
                checked.append(.init(selector: selector, before: pin.before, after: nil,
                                     verdict: .notCompared("the element is no longer on the page")))
                continue
            }
            guard rect.width > 1, rect.height > 1 else {
                checked.append(.init(selector: selector, before: pin.before, after: nil,
                                     verdict: .notCompared("the element is no longer visible")))
                continue
            }
            let after = await resnapshot(rect)
            checked.append(.init(
                selector: selector, before: pin.before, after: after,
                verdict: after == nil
                    ? .notCompared("the element is off-screen — scroll it into view to check it")
                    : DesignCheck.compare(before: pin.before, after: after)))
        }
        design.pins = checked
        turn.design = design
    }
}

/// What the workbench is showing beside the conversation: a detour into a diff, a file, a
/// commit or an inspection; a sheet; the turn both panes are focused on; the prompts that are
/// mid-answer. View state that two panes or the window read, per lane — so it is neither a
/// pane's own `@State` nor a fact about the project.
@MainActor
@Observable
final class WorkbenchViewModel {
    /// Where the stage is detoured to, if anywhere. One value: it was four optionals with a
    /// "first one wins" order written out at every site that read or cleared them.
    enum Detour {
        case diff(String)
        case file(String)
        case commit(Wire.Commit)
        case inspect(SessionModel.Inspect)
    }
    var detour: Detour?
    var sheet: SessionModel.Sheet?
    /// The turn the person clicked into, which both panes scroll to and the trace stops
    /// following for.
    var focusedTurn: UUID?
    var renamingSession: String?
    var renameDraft = ""
    var confirmingDiscard = false
}
