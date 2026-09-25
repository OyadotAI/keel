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
        let scroll = NSScrollView()
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
        if editor.string != text, !editor.hasMarkedText() {
            editor.string = text
            editor.setSelectedRange(NSRange(location: (text as NSString).length, length: 0))
            editor.undoManager?.removeAllActions()
        }
        editor.needsDisplay = true
        if focused, let window = editor.window, window.firstResponder !== editor {
            window.makeFirstResponder(editor)
        }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSScrollView, context: Context) -> CGSize? {
        guard let editor = context.coordinator.editor else { return nil }
        let width = max(1, proposal.width ?? 400)
        editor.textContainer?.containerSize = NSSize(width: width, height: .greatestFiniteMagnitude)
        guard let container = editor.textContainer, let manager = editor.layoutManager else { return nil }
        manager.ensureLayout(for: container)
        let height = max(54, manager.usedRect(for: container).height + 12)
        let visibleHeight = expanded ? 260 : min(160, height)
        editor.setFrameSize(NSSize(width: width, height: max(height, visibleHeight)))
        return CGSize(width: width, height: visibleHeight)
    }

    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: ComposerEditor
        weak var editor: ComposerTextView?
        init(_ parent: ComposerEditor) { self.parent = parent }
        func textDidChange(_ notification: Notification) {
            guard let editor else { return }
            parent.text = editor.string
            editor.needsDisplay = true
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
