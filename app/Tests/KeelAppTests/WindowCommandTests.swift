import XCTest
@testable import KeelApp

/// A menu key belongs to one window.
///
/// Every command in the app travels as a `NotificationCenter` post, so every open window used to
/// act on every one: ⌘N put the "New feature" sheet on two screens at once and filling in one left
/// the other standing. The exception is a command that names its lane by id — a click on a system
/// notification — which belongs to whichever window holds that lane, key or not.
final class WindowCommandTests: XCTestCase {
    func testOnlyTheKeyWindowActsOnAMenuCommand() {
        XCTAssertTrue(WindowCommand.acts(key: true, addressed: false, object: nil))
        XCTAssertFalse(WindowCommand.acts(key: false, addressed: false, object: nil))
    }

    /// ⌘1–9 sends an Int. It is still a menu key, so it is still the key window's.
    func testAnIndexIsStillAMenuCommand() {
        XCTAssertFalse(WindowCommand.acts(key: false, addressed: true, object: 3))
    }

    func testALaneIdReachesTheWindowThatHoldsIt() {
        XCTAssertTrue(WindowCommand.acts(key: false, addressed: true,
                                         object: UUID().uuidString))
    }
}
