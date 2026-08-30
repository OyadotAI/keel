import AppKit
import XCTest
@testable import KeelApp

/// What a drag onto the conversation is allowed to be.
///
/// The drop used to accept `.fileURL` alone, so an image dragged out of a browser — which arrives
/// as data with no file behind it — did nothing at all.
@MainActor
final class AttachmentTests: XCTestCase {
    private func model() -> SessionModel { SessionModel(client: Client(port: 0)) }

    func testAPictureWithNoFileBehindItStillBecomesAnAttachment() {
        let m = model()
        let image = NSImage(size: NSSize(width: 4, height: 4))
        image.lockFocus()
        NSColor.red.drawSwatch(in: NSRect(x: 0, y: 0, width: 4, height: 4))
        image.unlockFocus()

        // No daemon here, so the upload cannot finish — what is asserted is that the picture
        // survives being turned into PNG bytes, which is the step that used to be copy-pasted.
        XCTAssertNotNil(image.tiffRepresentation)
        m.attach(image: image, name: "dropped.png")
        XCTAssertNil(m.lastError)
    }

    func testTooBigIsRefusedHereRatherThanUploaded() {
        let m = model()
        m.attach(data: Data(count: SessionModel.maxAttachment + 1), name: "huge.bin")
        XCTAssertNotNil(m.lastError)
        XCTAssertTrue(m.attachments.isEmpty)
    }
}
