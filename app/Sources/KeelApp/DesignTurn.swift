import AppKit
import Foundation

/// The half of design mode nobody else ships: checking whether the change actually happened.
///
/// Cursor, Windsurf and Orca all pick an element, hand the agent some context, and then show you a
/// code diff. The cited failure is always the same — the agent guesses which source produced the
/// element, and when it guesses wrong it either edits nothing that matters or forks a new copy of
/// the component. A green diff looks identical in both cases.
///
/// So the pick is re-photographed afterwards and compared. Same rect, same page, same widths.
enum DesignCheck {
    /// What the pixels say happened.
    enum Verdict: Equatable {
        case changed
        /// The after image is identical to the before: whatever was edited, it was not this.
        case nothingChanged
        /// No comparison was made, and this is why.
        ///
        /// It used to be one case called `unstable`, rendered as "the page was still moving" —
        /// which nothing measured and which was almost never the truth. Closing the Designer tab
        /// produced it, so did an element scrolled out of view, so did an element the turn deleted,
        /// and all three read as if the page were mid-animation. A verdict that cannot say why it
        /// abstained is not a verdict.
        case notCompared(String)
    }

    /// Compare two snapshots of the same rect.
    ///
    /// Exact equality of the encoded bytes, not a perceptual diff: the question is only "did
    /// anything at all move", and a tolerance would answer a question nobody asked while quietly
    /// swallowing a one-pixel change someone did ask for.
    static func compare(before: NSImage?, after: NSImage?) -> Verdict {
        guard let before, let after,
              let b = png(before), let a = png(after) else {
            return .notCompared("there was nothing to compare")
        }
        return b == a ? .nothingChanged : .changed
    }

    static func png(_ image: NSImage) -> Data? {
        guard let tiff = image.tiffRepresentation else { return nil }
        return NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
    }

    /// Did the turn fork the component instead of editing it?
    ///
    /// The named failure mode — "it creates duplicates" — is visible in the file list: a turn asked
    /// to change an existing element that answers by adding a *new* component file has almost
    /// certainly copied rather than edited. A heuristic, and it says so.
    // ponytail: filename-shaped, not AST-shaped. A real check would compare the rendered subtree
    // before and after against the component that owns it.
    static func looksDuplicated(files: [String], hints: [Picked.Hint]) -> Bool {
        guard !hints.isEmpty else { return false }
        let named = hints.map { $0.value.split(separator: ":").first.map(String.init) ?? $0.value }
        let touchedAHint = files.contains { file in
            named.contains { hint in file.hasSuffix(hint) || hint.hasSuffix(file) }
        }
        let addedAComponent = files.contains { f in
            let base = (f as NSString).lastPathComponent
            return base.first?.isUppercase == true
                && [".tsx", ".jsx", ".svelte", ".vue"].contains { f.hasSuffix($0) }
        }
        return addedAComponent && !touchedAHint
    }
}
