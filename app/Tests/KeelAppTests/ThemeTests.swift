import AppKit
import SwiftUI
import XCTest
@testable import KeelApp

/// The palette has to resolve, differ between appearances, and stay readable.
///
/// This started as a check that asset-catalog names resolved — and immediately caught that six of
/// fifteen did not, because SwiftPM copies `.xcassets` without compiling it. The colours are
/// declared in code now, which removes that failure mode; what is left is worth keeping, because
/// a palette edit that quietly makes body text unreadable produces no error either.
final class ThemeTests: XCTestCase {

    /// Only a person stops a pane following its own tail.
    ///
    /// It used to unfollow whenever the pane read as short of the end, which a streaming reply
    /// makes true several times a second — so a turn scrolled itself out of sight and stayed
    /// there, and every message after that needed a manual scroll to read.
    func testContentGrowingUnderAPaneDoesNotStopItFollowing() {
        // Streaming: content grew, nobody touched the wheel. Still following.
        XCTAssertTrue(FollowsTail.following(true, atBottom: false, byHand: false))
        // Scrolled up by hand: stop.
        XCTAssertFalse(FollowsTail.following(true, atBottom: false, byHand: true))
        // Back at the end, however you got there: follow again.
        XCTAssertTrue(FollowsTail.following(false, atBottom: true, byHand: true))
        XCTAssertTrue(FollowsTail.following(false, atBottom: true, byHand: false))
        // Away from the end and already unfollowed: stay unfollowed.
        XCTAssertFalse(FollowsTail.following(false, atBottom: false, byHand: false))
    }

    /// The white pane: the transcript scrolled clean off the end of its own content.
    ///
    /// Reported repeatedly as "the middle goes empty, I scroll up and back down and it is there".
    /// `scrollPosition(id:)` holds a row, and a replay rebuilds every `Turn` with a new id — so
    /// the anchor names nothing, the offset stays where it was, and the content it was measured
    /// against is gone. The threshold is the whole viewport past the end, which a rubber band
    /// cannot reach, so a fling never trips it.
    func testAPaneScrolledOffItsOwnContentIsRecognisedAsBlank() {
        XCTAssertTrue(ChatRail.blank(offset: 4000, content: 1200, viewport: 800))
        XCTAssertTrue(ChatRail.blank(offset: 1200, content: 1200, viewport: 800))
        // The state people actually report: a sliver of the last turn at the top, the rest of
        // the window white. Past the legal maximum offset of 400, so no scroll made it.
        XCTAssertTrue(ChatRail.blank(offset: 1100, content: 1200, viewport: 800))
        // At rest at the end, and a rubber band a quarter of the way past it.
        XCTAssertFalse(ChatRail.blank(offset: 400, content: 1200, viewport: 800))
        XCTAssertFalse(ChatRail.blank(offset: 600, content: 1200, viewport: 800))
        XCTAssertFalse(ChatRail.blank(offset: 0, content: 1200, viewport: 800))
        // Nothing laid out yet is not a blank pane, it is a pane that has not been measured.
        XCTAssertFalse(ChatRail.blank(offset: 0, content: 0, viewport: 800))
        // A conversation shorter than the window cannot scroll away from itself.
        XCTAssertFalse(ChatRail.blank(offset: 0, content: 200, viewport: 800))
    }

    private func resolve(_ color: Color, _ appearance: NSAppearance) -> NSColor {
        var out = NSColor.black
        appearance.performAsCurrentDrawingAppearance {
            out = NSColor(color).usingColorSpace(.sRGB) ?? .black
        }
        return out
    }

    private var light: NSAppearance { NSAppearance(named: .aqua)! }
    private var dark: NSAppearance { NSAppearance(named: .darkAqua)! }

    /// A token that renders the same in both appearances is a dark mode that is just light mode.
    func testEveryTokenDiffersBetweenAppearances() {
        for (name, color) in K.C.all {
            let l = resolve(color, light)
            let d = resolve(color, dark)
            XCTAssertNotEqual(l.redComponent, d.redComponent, accuracy: 0.001,
                              "`\(name)` is identical in light and dark")
        }
    }

    /// Text on its own ground has to be readable. A floor, not a full WCAG pass.
    func testTextContrastsWithItsGround() {
        let grounds: [(String, Color)] = [("bg", K.C.bg), ("surface", K.C.surface),
                                          ("raised", K.C.raised), ("well", K.C.well)]
        let inks: [(String, Color)] = [("text", K.C.text), ("dim", K.C.dim)]

        for appearance in [light, dark] {
            for (gName, ground) in grounds {
                for (iName, ink) in inks {
                    let ratio = contrast(resolve(ink, appearance), resolve(ground, appearance))
                    XCTAssertGreaterThan(ratio, 4.5,
                        "`\(iName)` on `\(gName)` is \(String(format: "%.1f", ratio)):1, "
                        + "below the 4.5:1 floor")
                }
            }
        }
    }

    /// Diff row grounds must stay *weak*. The temptation is to make them obvious; forty changed
    /// lines on a saturated ground is unreadable, so this fails if they get loud.
    func testDiffGroundsStayQuiet() {
        for appearance in [light, dark] {
            for (name, ground) in [("addBG", K.C.addBG), ("delBG", K.C.delBG)] {
                let ratio = contrast(resolve(ground, appearance), resolve(K.C.bg, appearance))
                XCTAssertLessThan(ratio, 1.6,
                    "`\(name)` is \(String(format: "%.2f", ratio)):1 against the ground — "
                    + "loud enough to fight the code on top of it")
            }
        }
    }

    /// The semantic colours have to be distinguishable from body text, or a failure reads as prose.
    func testSemanticColoursAreDistinct() {
        for appearance in [light, dark] {
            let text = resolve(K.C.text, appearance)
            for (name, color) in [("add", K.C.add), ("del", K.C.del), ("warn", K.C.warn),
                                  ("accent", K.C.accent)] {
                let c = resolve(color, appearance)
                let distance = abs(c.redComponent - text.redComponent)
                    + abs(c.greenComponent - text.greenComponent)
                    + abs(c.blueComponent - text.blueComponent)
                XCTAssertGreaterThan(distance, 0.25,
                                     "`\(name)` is too close to body text to read as a signal")
            }
        }
    }

    private func contrast(_ a: NSColor, _ b: NSColor) -> Double {
        let la = luminance(a), lb = luminance(b)
        return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
    }

    private func luminance(_ c: NSColor) -> Double {
        func ch(_ v: CGFloat) -> Double {
            let v = Double(v)
            return v <= 0.03928 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * ch(c.redComponent) + 0.7152 * ch(c.greenComponent) + 0.0722 * ch(c.blueComponent)
    }
}


/// Nothing meant to be read is set below the platform's floor.
///
/// macOS's small-system size is 11 and the HIG's captions stop at 10. The tokens claimed 11 in
/// their doc comments while one of them was 10, and forty call sites went lower still.
final class TypeFloorTests: XCTestCase {
    func testNoNamedSizeIsBelowTheFloor() {
        for (name, size) in K.F.sizes {
            XCTAssertGreaterThanOrEqual(size, K.F.floor, "K.F.\(name) is \(size)")
        }
    }

    /// The sources themselves: no inline text size under the floor, so the tokens cannot be
    /// walked around with a literal.
    func testNoInlineSizeIsBelowTheFloor() throws {
        var dir = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        for _ in 0..<4 {
            let src = dir.appendingPathComponent("app/Sources/KeelApp")
            let alt = dir.appendingPathComponent("Sources/KeelApp")
            if let found = [src, alt].first(where: { FileManager.default.fileExists(atPath: $0.path) }) {
                try check(found); return
            }
            dir = dir.deletingLastPathComponent()
        }
        throw XCTSkip("sources not found from \(FileManager.default.currentDirectoryPath)")
    }

    private func check(_ dir: URL) throws {
        let files = try FileManager.default.subpathsOfDirectory(atPath: dir.path)
            .filter { $0.hasSuffix(".swift") }
        let pattern = try NSRegularExpression(pattern: #"(?:size: |mono\()(\d+(?:\.\d+)?)"#)
        for f in files {
            let lines = try String(contentsOf: dir.appendingPathComponent(f), encoding: .utf8)
                .components(separatedBy: .newlines)
            for (i, line) in lines.enumerated() {
                // The modifier is often on a continuation line under the `Image` it decorates, so
                // the exemption reads the line above too.
                let context = (i > 0 ? lines[i - 1] : "") + line
                guard !isGlyph(context) else { continue }
                for m in pattern.matches(in: line, range: NSRange(line.startIndex..., in: line)) {
                    let n = Double(line[Range(m.range(at: 1), in: line)!])!
                    XCTAssertGreaterThanOrEqual(n, Double(K.F.floor), "\(f):\(i + 1): size \(n)")
                }
            }
        }
    }

    /// Nothing outside `Theme.swift` builds its own font.
    ///
    /// The floor test above has always existed and the drift happened anyway, because it only
    /// checked that a literal was not *too small* — never that it went through the scale. 147
    /// `.font(.system(size:))` calls passed it. Every size a view needs is a token or a call to
    /// `K.F.ui`/`K.F.mono`, and this is what makes that true tomorrow rather than only today.
    func testNoViewBuildsItsOwnFont() throws {
        try eachSourceLine { file, n, line in
            guard file != "Theme.swift" else { return }
            XCTAssertFalse(line.contains(".system(size:"),
                           "\(file):\(n): builds a font by hand — use a `K.F` token")
            XCTAssertFalse(line.contains(".font(.caption") || line.contains(".font(.body)")
                           || line.contains(".font(.title") || line.contains(".font(.headline"),
                           "\(file):\(n): uses a stock text style — use a `K.F` token")
        }
    }

    /// Nothing outside `Theme.swift` defines a button style.
    ///
    /// `QuietButton` lived in `TurnStage.swift` at 74 call sites, and two byte-identical copies of
    /// the primary button lived in `ChatRail` and `Welcome`, each documented as the only one. A
    /// component nobody can find is a component that gets written again.
    func testEveryButtonStyleLivesInTheSystem() throws {
        try eachSourceLine { file, n, line in
            guard file != "Theme.swift" else { return }
            XCTAssertFalse(line.contains(": ButtonStyle {"),
                           "\(file):\(n): defines a button style outside the design system")
        }
    }

    /// Padding and spacing come off the scale.
    ///
    /// `K.S` grew `hair`, `tight` and `snug` precisely so this could be true — the 4pt grid could
    /// not express what a dense row wants, so 1, 3 and 5 were written out about 120 times.
    func testSpacingComesOffTheScale() throws {
        let pattern = try NSRegularExpression(
            pattern: #"(?:\.padding\((?:\.\w+, )?|spacing: )(\d+(?:\.\d+)?)[,)]"#)
        try eachSourceLine { file, n, line in
            guard file != "Theme.swift" else { return }
            for m in pattern.matches(in: line, range: NSRange(line.startIndex..., in: line)) {
                let v = Double(line[Range(m.range(at: 1), in: line)!])!
                // Zero is a real answer — a list with no gap between its rows.
                XCTAssertEqual(v, 0, accuracy: 0.001,
                               "\(file):\(n): \(v) is off the `K.S` scale")
            }
        }
    }

    /// An icon-only control announces itself.
    ///
    /// `hint()` is `.help()` plus `.accessibilityLabel`, and it exists because thirty tooltips were
    /// pointer-only: the strings were written, they just never reached VoiceOver. `.help()` alone
    /// on a button whose label is a glyph leaves it as an unnamed button.
    func testIconOnlyControlsAreNamed() throws {
        let dir = try sourceDirectory()
        for f in try FileManager.default.subpathsOfDirectory(atPath: dir.path)
            .filter({ $0.hasSuffix(".swift") }) where f != "Theme.swift" {
            let lines = try String(contentsOf: dir.appendingPathComponent(f), encoding: .utf8)
                .components(separatedBy: .newlines)
            for (i, line) in lines.enumerated() where line.contains(".help(") {
                // The label is whatever the control renders. A glyph and no text is the case
                // `hint()` was written for; anything with a `Text` already names itself.
                let above = lines[max(0, i - 8)..<i].joined(separator: "\n")
                let iconOnly = above.contains("Image(systemName:")
                    && !above.contains("Text(") && !above.contains("Label(")
                XCTAssertFalse(iconOnly,
                               "\(f):\(i + 1): icon-only control uses `.help(` — use `.hint(`, "
                               + "which is the tooltip *and* the VoiceOver label")
            }
        }
    }

    private func sourceDirectory() throws -> URL {
        var dir = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        for _ in 0..<4 {
            for candidate in ["app/Sources/KeelApp", "Sources/KeelApp"] {
                let path = dir.appendingPathComponent(candidate)
                if FileManager.default.fileExists(atPath: path.path) { return path }
            }
            dir = dir.deletingLastPathComponent()
        }
        throw XCTSkip("sources not found from \(FileManager.default.currentDirectoryPath)")
    }

    /// Every line of every source file, with its name and number, skipping comments — a rule about
    /// what the code does should not fire on a doc comment describing the rule.
    private func eachSourceLine(_ each: (String, Int, String) throws -> Void) throws {
        let dir = try sourceDirectory()
        for f in try FileManager.default.subpathsOfDirectory(atPath: dir.path)
            .filter({ $0.hasSuffix(".swift") }) {
            let text = try String(contentsOf: dir.appendingPathComponent(f), encoding: .utf8)
            for (i, line) in text.components(separatedBy: .newlines).enumerated() {
                let code = line.trimmingCharacters(in: .whitespaces)
                guard !code.hasPrefix("//") else { continue }
                try each(f, i + 1, line)
            }
        }
    }

    /// Glyph-only sizes — a chevron, a dot, a close ✕ — are allowed under the floor; text is not.
    ///
    /// This used to be `if n >= 8`, a stand-in for "below 8 nothing is text". It was wrong in both
    /// directions: it let a 7pt *label* through, and it failed a 9pt chevron and a
    /// `CloseButton(size: 9)` — which is not a font size at all, just a parameter that shares the
    /// name. Naming the exemption is both more permissive and more strict than guessing at it.
    private func isGlyph(_ context: String) -> Bool {
        ["Image(systemName:", "CloseButton(", "Circle()", "circle.fill", "chevron."]
            .contains { context.contains($0) }
    }
}
