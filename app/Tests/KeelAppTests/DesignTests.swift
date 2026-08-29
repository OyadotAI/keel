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


/// The live canvas: which files count, which page they are, and how pins become a prompt.
@MainActor
final class CanvasTests: XCTestCase {
    func testWhatCountsAsTheFrontend() {
        XCTAssertTrue(Frontend.isUI("frontend/app/page.tsx"))
        XCTAssertTrue(Frontend.isUI("src/components/Button.vue"))
        XCTAssertTrue(Frontend.isUI("app/globals.css"))
        XCTAssertFalse(Frontend.isUI("backend/src/index.ts"))
        XCTAssertFalse(Frontend.isUI("src/api/users.ts"))
        XCTAssertFalse(Frontend.isUI("src/Button.test.tsx"))
        XCTAssertFalse(Frontend.isUI("next.config.ts"))
        XCTAssertFalse(Frontend.isUI("README.md"))
    }

    /// A page file is a URL when there is exactly one; a dynamic segment is not.
    func testAPageFileIsARoute() {
        XCTAssertEqual(Frontend.route(for: "frontend/app/page.tsx"), "/")
        XCTAssertEqual(Frontend.route(for: "app/pricing/page.tsx"), "/pricing")
        XCTAssertEqual(Frontend.route(for: "app/(marketing)/about/page.tsx"), "/about")
        XCTAssertNil(Frontend.route(for: "app/blog/[slug]/page.tsx"))
        XCTAssertNil(Frontend.route(for: "app/components/Header.tsx"), "a component is not a page")
        XCTAssertEqual(Frontend.route(for: "pages/index.tsx"), "/")
        XCTAssertEqual(Frontend.route(for: "pages/docs/intro.tsx"), "/docs/intro")
        XCTAssertNil(Frontend.route(for: "pages/api/hello.ts"))
        XCTAssertNil(Frontend.route(for: "pages/_app.tsx"))
        XCTAssertEqual(Frontend.route(for: "src/routes/settings/+page.svelte"), "/settings")
        XCTAssertEqual(Frontend.origin(of: "http://127.0.0.1:3000/x/y?z"), "http://127.0.0.1:3000")
    }

    private func picked(_ selector: String, text: String = "") -> Picked {
        let json = """
        {"selector":"\(selector)","tag":"button","text":"\(text)","html":"<button/>","style":{},
         "hints":[{"kind":"component","value":"Header"}],"rect":{"x":1,"y":2,"width":30,"height":10},"dpr":2}
        """
        return try! JSONDecoder().decode(Picked.self, from: Data(json.utf8))
    }

    /// Several pins become one prompt, numbered, each carrying its note.
    func testPinsBecomeOnePrompt() {
        let m = SessionModel(client: Client(port: 0))
        m.designPick(picked("#a", text: "Sign up"), before: nil)
        m.designPick(picked("#b"), before: nil)
        m.pins[0].note = "make it green"
        let p = m.designPrompt("and keep the layout")!
        XCTAssertTrue(p.contains("## Pin 1"))
        XCTAssertTrue(p.contains("## Pin 2"))
        XCTAssertTrue(p.contains("Note on this element: make it green"))
        XCTAssertTrue(p.hasSuffix("and keep the layout"))
        // Picking the same element twice focuses the pin rather than stacking a second.
        m.designPick(picked("#a"), before: nil)
        XCTAssertEqual(m.pins.count, 2)
    }

    /// The page's report decodes, and the union rect covers all of it.
    func testChangedRegionsDecodeAndUnion() throws {
        let json = #"{"type":"changed","regions":[{"selector":"nav","rect":{"x":0,"y":0,"width":100,"height":20},"tag":"nav","text":"Home"},{"selector":"main > p","rect":{"x":10,"y":50,"width":50,"height":30},"tag":"p","text":""}]}"#
        struct Changed: Decodable { var regions: [Region] }
        let c = try JSONDecoder().decode(Changed.self, from: Data(json.utf8))
        XCTAssertEqual(c.regions.count, 2)
        let u = Picked.Rect.union(c.regions.map(\.rect))!
        XCTAssertEqual(u.x, 0); XCTAssertEqual(u.y, 0)
        XCTAssertEqual(u.width, 100); XCTAssertEqual(u.height, 80)
    }

    /// A write to the frontend mid-turn is what moves the preview; a backend write is not.
    func testAFrontendWriteMovesThePreview() {
        let m = SessionModel(client: Client(port: 0)), t = Turn(prompt: "add pricing")
        m.previewURL = "http://127.0.0.1:3000/"
        var sent: [String] = []
        m.canvas = { sent.append($0["keel"] as? String ?? "") }
        let tick = m.designTick
        m.record(Data(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"w1","name":"Write","input":{"file_path":"/repo/frontend/app/pricing/page.tsx"}}]}}"#.utf8), into: t)
        XCTAssertEqual(m.editing, "/repo/frontend/app/pricing/page.tsx")
        XCTAssertEqual(m.previewURL, "http://127.0.0.1:3000/pricing", "navigated to the page")
        XCTAssertEqual(sent.last, "expect")
        XCTAssertGreaterThan(m.designTick, tick)

        m.record(Data(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"w2","name":"Write","input":{"file_path":"/repo/backend/src/index.ts"}}]}}"#.utf8), into: t)
        XCTAssertEqual(m.editing, "/repo/frontend/app/pricing/page.tsx", "a backend write changes nothing")
    }
}
