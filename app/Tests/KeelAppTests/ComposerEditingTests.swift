import AppKit
import SwiftUI
import XCTest
@testable import KeelApp

@MainActor
final class ComposerEditingTests: XCTestCase {
    private func key(_ code: UInt16, modifiers: NSEvent.ModifierFlags = []) -> NSEvent {
        NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: modifiers,
                        timestamp: 0, windowNumber: 0, context: nil, characters: "\r",
                        charactersIgnoringModifiers: "\r", isARepeat: false, keyCode: code)!
    }

    func testReturnAcceptsCompletionBeforeItCanSend() {
        let editor = ComposerTextView()
        var accepted = 0
        var sent = 0
        editor.navigate = { direction in
            guard direction == .accept, accepted == 0 else { return false }
            accepted += 1
            return true
        }
        editor.submit = { sent += 1 }
        editor.keyDown(with: key(36))
        XCTAssertEqual(accepted, 1)
        XCTAssertEqual(sent, 0)
        editor.keyDown(with: key(36))
        XCTAssertEqual(sent, 1)
    }

    func testShiftReturnInsertsAtSelectionWithoutSending() {
        let editor = ComposerTextView()
        editor.string = "before after"
        editor.setSelectedRange(NSRange(location: 6, length: 1))
        var sent = false
        editor.submit = { sent = true }
        editor.keyDown(with: key(36, modifiers: .shift))
        XCTAssertEqual(editor.string, "before\nafter")
        XCTAssertFalse(sent)
    }

    func testNativeInsertionReplacesSelectionAndSupportsUndo() {
        let editor = ComposerTextView()
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 400, height: 200),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.contentView = editor
        editor.allowsUndo = true
        editor.string = "hello world"
        editor.setSelectedRange(NSRange(location: 6, length: 5))
        editor.insertText("Keel", replacementRange: editor.selectedRange())
        XCTAssertEqual(editor.string, "hello Keel")
        editor.undoManager?.undo()
        XCTAssertEqual(editor.string, "hello world")
        window.contentView = nil
    }

    func testPlainTextPasteUsesTheCurrentSelection() {
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        board.setString("replacement", forType: .string)
        let editor = ComposerTextView()
        editor.isRichText = false
        editor.string = "before old after"
        editor.setSelectedRange(NSRange(location: 7, length: 3))
        XCTAssertTrue(editor.readSelection(from: board, type: .string))
        XCTAssertEqual(editor.string, "before replacement after")
    }

    func testFocusTracksResponderChangesBeforeTyping() async {
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 400, height: 200),
                              styleMask: [.titled], backing: .buffered, defer: false)
        let editor = ComposerTextView()
        window.contentView = editor
        var focused = false
        editor.focusChanged = { focused = $0 }
        XCTAssertTrue(window.makeFirstResponder(editor))
        await Task.yield()
        try? await Task.sleep(for: .milliseconds(20))
        XCTAssertTrue(focused)
        XCTAssertTrue(window.makeFirstResponder(nil))
        await Task.yield()
        try? await Task.sleep(for: .milliseconds(20))
        XCTAssertFalse(focused)
        window.contentView = nil
    }

    func testExpandedEditorKeepsSelectionWhenResized() {
        let text = String(repeating: "A useful draft with several wrapped lines. ", count: 15)
        let host = NSHostingView(rootView: ComposerEditor(text: .constant(text), focused: .constant(false),
            expanded: true, submit: {}, navigate: { _ in false }, pasteAttachment: { _ in false }))
        host.frame = NSRect(x: 0, y: 0, width: 600, height: 300)
        host.layoutSubtreeIfNeeded()
        func find(_ view: NSView) -> ComposerTextView? {
            (view as? ComposerTextView) ?? view.subviews.compactMap { find($0) }.first
        }
        guard let editor = find(host) else { return XCTFail("Missing native editor") }
        editor.setSelectedRange(NSRange(location: 9, length: 5))
        host.frame.size.width = 340
        host.layoutSubtreeIfNeeded()
        XCTAssertEqual(editor.selectedRange(), NSRange(location: 9, length: 5))
        XCTAssertEqual(editor.string, text)
        XCTAssertGreaterThanOrEqual(editor.frame.height, 260)
    }

    func testEditingQueuedMessagePreservesDraftAndOtherQueuedMessages() {
        let model = SessionModel(client: Client(port: 0))
        model.prompt = "Current draft"
        model.queued = ["First", "Second"]
        model.restoreQueued(at: 0)
        XCTAssertEqual(model.prompt, "Current draft\n\nFirst")
        XCTAssertEqual(model.queued, ["Second"])
        model.restoreQueued(at: 99)
        XCTAssertEqual(model.queued, ["Second"])
    }

    func testWhitespaceCannotBeSent() {
        let model = SessionModel(client: Client(port: 0))
        model.prompt = " \n "
        XCTAssertFalse(model.canSendDraft)
        model.prompt = "Review these changes"
        XCTAssertTrue(model.canSendDraft)
    }
}
