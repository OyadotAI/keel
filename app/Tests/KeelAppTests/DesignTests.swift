import AppKit
import XCTest
@testable import KeelApp

/// The verification half of design mode, which is the part that is actually novel.
final class DesignTests: XCTestCase {

    private func image(_ color: NSColor, size: CGFloat = 8) -> NSImage {
        let img = NSImage(size: NSSize(width: size, height: size))
        img.lockFocus()
        color.setFill()
        NSRect(x: 0, y: 0, width: size, height: size).fill()
        img.unlockFocus()
        return img
    }

    /// The whole point. An edit that hit the wrong file leaves the pixels alone, and Keel has to
    /// say so rather than letting a green diff imply the change landed.
    func testIdenticalPixelsMeanNothingChanged() {
        let a = image(.red)
        XCTAssertEqual(DesignCheck.compare(before: a, after: image(.red)), .nothingChanged)
    }

    func testDifferentPixelsMeanItChanged() {
        XCTAssertEqual(DesignCheck.compare(before: image(.red), after: image(.blue)), .changed)
    }

    /// A missing snapshot is reported as unstable, never as success. Failing the other way would
    /// turn "we could not photograph it" into "it worked".
    func testAMissingSnapshotIsNotAPass() {
        XCTAssertEqual(DesignCheck.compare(before: image(.red), after: nil), .unstable)
        XCTAssertEqual(DesignCheck.compare(before: nil, after: nil), .unstable)
    }

    /// The named failure mode elsewhere: asked to change a button, the agent writes a new one.
    func testANewComponentThatIgnoresTheHintLooksDuplicated() {
        let hints = [Picked.Hint(kind: "component", value: "PrimaryButton")]
        XCTAssertTrue(DesignCheck.looksDuplicated(
            files: ["src/components/NewButton.tsx"], hints: hints))
    }

    func testEditingTheHintedFileIsNotDuplication() {
        let hints = [Picked.Hint(kind: "attribute", value: "src/components/PrimaryButton.tsx:14")]
        XCTAssertFalse(DesignCheck.looksDuplicated(
            files: ["src/components/PrimaryButton.tsx"], hints: hints))
    }

    /// With nothing to compare against, the heuristic must stay quiet rather than accuse.
    func testNoHintsMeansNoAccusation() {
        XCTAssertFalse(DesignCheck.looksDuplicated(files: ["src/Anything.tsx"], hints: []))
    }
}

/// The changes tree, which is the panel people navigate a review from.
@MainActor
final class ChangeTreeTests: XCTestCase {

    private func change(_ path: String, _ status: String = " M") -> Wire.Change {
        Wire.Change(path: path, status: status, label: "modified")
    }

    /// Files sharing a directory sit under it once, rather than repeating the prefix per row.
    func testFilesGroupUnderTheirDirectory() {
        let tree = ChangeTree.build([
            change("crates/keel/src/api.rs"),
            change("crates/keel/src/serve.rs"),
            change("README.md"),
        ])
        // A chain of single-child folders collapses, so this is one row, not three.
        let dir = tree.first { $0.isDir }
        XCTAssertEqual(dir?.name, "crates/keel/src")
        XCTAssertEqual(dir?.children.map(\.name), ["api.rs", "serve.rs"])
        XCTAssertTrue(tree.contains { $0.name == "README.md" && !$0.isDir })
    }

    /// A branch point must stop the collapse, or two sibling directories merge into a lie.
    func testACollapseStopsWhereTheTreeBranches() {
        let tree = ChangeTree.build([
            change("app/Sources/One.swift"),
            change("app/Tests/Two.swift"),
        ])
        XCTAssertEqual(tree.count, 1)
        XCTAssertEqual(tree[0].name, "app")
        XCTAssertEqual(tree[0].children.map(\.name).sorted(), ["Sources", "Tests"])
    }

    /// Folders before files, each alphabetically — the order every file browser uses.
    func testFoldersSortBeforeFiles() {
        let tree = ChangeTree.build([
            change("zebra.txt"),
            change("alpha/one.txt"),
            change("apple.txt"),
        ])
        XCTAssertEqual(tree.map(\.name), ["alpha", "apple.txt", "zebra.txt"])
    }

    /// A folder row says how many files are under it, however deep.
    func testAFolderCountsEveryFileBeneathIt() {
        let tree = ChangeTree.build([
            change("a/b/one.txt"),
            change("a/c/two.txt"),
            change("a/c/three.txt"),
        ])
        XCTAssertEqual(tree.count, 1)
        XCTAssertEqual(tree[0].fileCount, 3)
    }

    /// A file at the root has no directory to hang from and must not be dropped.
    func testRootLevelFilesSurvive() {
        let tree = ChangeTree.build([change("Makefile"), change("CLAUDE.md")])
        XCTAssertEqual(tree.map(\.name), ["CLAUDE.md", "Makefile"])
        XCTAssertTrue(tree.allSatisfy { !$0.isDir })
    }
}

/// The preview address bar.
@MainActor
final class PreviewURLTests: XCTestCase {

    /// What someone types is not a URL yet. `localhost:3000` parses as a URL whose *scheme* is
    /// `localhost`, so it never throws and never loads — it just silently shows nothing.
    func testWhatPeopleActuallyType() {
        XCTAssertEqual(PreviewSurface.normalise("localhost:3000"), "http://localhost:3000")
        XCTAssertEqual(PreviewSurface.normalise("3000"), "http://127.0.0.1:3000")
        XCTAssertEqual(PreviewSurface.normalise("  localhost:3000  "), "http://localhost:3000")
        XCTAssertEqual(PreviewSurface.normalise("127.0.0.1:8080/app"), "http://127.0.0.1:8080/app")
    }

    /// An address that is already one is left alone, including https.
    func testAnAddressIsLeftAlone() {
        XCTAssertEqual(PreviewSurface.normalise("https://example.com"), "https://example.com")
        XCTAssertEqual(PreviewSurface.normalise("http://localhost:3000"), "http://localhost:3000")
    }

    func testEmptyStaysEmpty() {
        XCTAssertEqual(PreviewSurface.normalise("   "), "")
    }

    /// Only local addresses are picked up from command output: a docs link printed by some tool
    /// must not repoint the preview at a website.
    func testOnlyLocalURLsAreNoticed() {
        let m = SessionModel(client: Client(port: 0), port: 0)
        m.noticeURL(in: "See https://docs.example.com/guide for details")
        XCTAssertNil(m.previewURL)

        m.noticeURL(in: "  ➜  Local:   http://localhost:5173/")
        XCTAssertEqual(m.previewURL, "http://localhost:5173/")
    }

    /// Keel's own daemon is not the app you wanted to look at.
    func testKeelsOwnPortIsIgnored() {
        let m = SessionModel(client: Client(port: 0), port: 0)
        m.noticeURL(in: "listening on http://127.0.0.1:7777")
        XCTAssertNil(m.previewURL)
    }
}
