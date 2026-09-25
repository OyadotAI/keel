import AppKit
import SwiftUI

/// A real text editor keeps selection, undo, input methods and paste inside AppKit.
/// SwiftUI only synchronizes external draft changes and the measured editor height.
struct ComposerEditor: NSViewRepresentable {
    @Binding var text: String
    @Binding var focused: Bool
    var expanded: Bool
    var submit: () -> Void
    var navigate: (ComposerNavigation) -> Bool
    var pasteAttachment: (NSPasteboard) -> Bool

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = ComposerScrollView()
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.borderType = .noBorder
        let editor = ComposerTextView(frame: .zero)
        editor.isRichText = false
        editor.allowsUndo = true
        editor.isAutomaticQuoteSubstitutionEnabled = false
        editor.isAutomaticDashSubstitutionEnabled = false
        editor.isAutomaticTextReplacementEnabled = false
        editor.isVerticallyResizable = true
        editor.isHorizontallyResizable = false
        editor.minSize = .zero
        editor.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        editor.autoresizingMask = [.width]
        editor.textContainer?.widthTracksTextView = true
        editor.textContainer?.lineFragmentPadding = 0
        editor.textContainerInset = NSSize(width: 0, height: 4)
        editor.drawsBackground = false
        editor.font = K.F.editor
        editor.textColor = NSColor(K.C.text)
        editor.insertionPointColor = NSColor(K.C.accent)
        editor.delegate = context.coordinator
        editor.setAccessibilityLabel("Message to Keel")
        scroll.documentView = editor
        context.coordinator.editor = editor
        updateNSView(scroll, context: context)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self
        guard let editor = coordinator.editor else { return }
        editor.submit = submit
        editor.navigate = navigate
        editor.pasteAttachment = pasteAttachment
        editor.focusChanged = { [weak coordinator] value in
            guard let coordinator, coordinator.parent.focused != value else { return }
            coordinator.parent.focused = value
        }
        // Only when the draft actually moved. This runs on every model change — during a turn,
        // several times a second — and an unconditional redraw was part of the flicker.
        if editor.string != text, !editor.hasMarkedText() {
            editor.string = text
            editor.setSelectedRange(NSRange(location: (text as NSString).length, length: 0))
            editor.undoManager?.removeAllActions()
            editor.needsDisplay = true
        }
        if focused, let window = editor.window, window.firstResponder !== editor {
            window.makeFirstResponder(editor)
        }
    }

    /// A question, not an instruction. SwiftUI asks this several times per layout with trial
    /// widths — 400, the minimum, the ideal — and this used to answer by resizing the real editor
    /// to each of them. The last trial won, so the draft wrapped at half the composer, and during
    /// a turn every model change asked again with a different width and the text jumped. The
    /// measurement happens on a scratch layout now; the editor takes its width from the scroll
    /// view it actually sits in (`ComposerScrollView`).
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSScrollView, context: Context) -> CGSize? {
        let width = max(1, proposal.width ?? 400)
        let draft = context.coordinator.editor?.string ?? text
        let height = max(54, context.coordinator.measure(draft, width: width) + 12)
        return CGSize(width: width, height: expanded ? 260 : min(160, height))
    }

    @MainActor
    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: ComposerEditor
        weak var editor: ComposerTextView?
        init(_ parent: ComposerEditor) { self.parent = parent }

        private var measured: (text: String, width: CGFloat, height: CGFloat)?

        /// The draft's height at a width, laid out off to the side. Remembered for the last
        /// answer, because the same question arrives several times per layout pass.
        func measure(_ text: String, width: CGFloat) -> CGFloat {
            if let m = measured, m.width == width, m.text == text { return m.height }
            let storage = NSTextStorage(string: text, attributes: [.font: K.F.editor])
            let container = NSTextContainer(size: NSSize(width: width, height: .greatestFiniteMagnitude))
            container.lineFragmentPadding = 0
            let manager = NSLayoutManager()
            manager.addTextContainer(container)
            storage.addLayoutManager(manager)
            manager.ensureLayout(for: container)
            // A trailing newline is a line of its own, and `usedRect` leaves it out.
            let height = max(manager.usedRect(for: container).maxY, manager.extraLineFragmentRect.maxY) + 8
            measured = (text, width, height)
            return height
        }
        func textDidChange(_ notification: Notification) {
            guard let editor else { return }
            parent.text = editor.string
            editor.needsDisplay = true
        }
    }
}

/// Keeps the editor exactly as wide as the space it is shown in, and at least as tall.
final class ComposerScrollView: NSScrollView {
    override func tile() {
        super.tile()
        guard let editor = documentView as? NSTextView else { return }
        let size = contentSize
        editor.minSize = NSSize(width: 0, height: size.height)
        if editor.frame.width != size.width {
            editor.setFrameSize(NSSize(width: size.width, height: max(editor.frame.height, size.height)))
        }
    }
}

enum ComposerNavigation: Equatable { case previous, next, accept, dismiss }

/// Key handling leaves marked text and selection to the platform; completion never sends a turn.
final class ComposerTextView: NSTextView {
    var submit: () -> Void = {}
    var navigate: (ComposerNavigation) -> Bool = { _ in false }
    var pasteAttachment: (NSPasteboard) -> Bool = { _ in false }
    var focusChanged: (Bool) -> Void = { _ in }

    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder()
        if accepted { reportFocus() }
        return accepted
    }

    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder()
        if accepted { reportFocus() }
        return accepted
    }

    /// First-responder changes can occur while SwiftUI is updating the view. Report the settled
    /// native state on the next turn so toolbar clicks cannot steal focus back into the editor.
    private func reportFocus() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.focusChanged(self.window?.firstResponder === self)
        }
    }

    override func keyDown(with event: NSEvent) {
        guard !hasMarkedText() else { super.keyDown(with: event); return }
        let modifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if event.keyCode == 36 || event.keyCode == 76 {
            if modifiers.contains(.shift) { insertNewlineIgnoringFieldEditor(nil); return }
            if !modifiers.contains(.command), navigate(.accept) { return }
            submit()
            return
        }
        if modifiers.isDisjoint(with: [.command, .option, .control, .shift]) {
            let direction: ComposerNavigation? = switch event.keyCode {
            case 126: .previous
            case 125: .next
            case 48: .accept
            case 53: .dismiss
            default: nil
            }
            if let direction, navigate(direction) { return }
        }
        super.keyDown(with: event)
    }

    override func paste(_ sender: Any?) {
        if !pasteAttachment(.general) { super.paste(sender) }
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        if string.isEmpty {
            ("Ask Keel to build, explain, or review…" as NSString).draw(
                at: NSPoint(x: 0, y: 4),
                withAttributes: [.font: K.F.editor, .foregroundColor: NSColor(K.C.dim)])
        }
    }
}
