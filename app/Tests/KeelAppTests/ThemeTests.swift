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
