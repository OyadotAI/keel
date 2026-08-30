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
        XCTAssertEqual(DesignCheck.compare(before: image(.red), after: nil),
                       .notCompared("there was nothing to compare"))
        XCTAssertEqual(DesignCheck.compare(before: nil, after: nil),
                       .notCompared("there was nothing to compare"))
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

/// The wire contracts that are two structs in two languages agreeing about the same bytes.
final class WireContractTests: XCTestCase {

    /// The daemon's `RenameBody` requires `name`. This was sent as `title`, axum answered 422,
    /// and `try?` swallowed it — so renaming a tab renamed the tab and nothing else, and people
    /// went looking in History for a name that was never written.
    func testARenameSendsTheFieldTheDaemonRequires() throws {
        let body = SessionModel.SessionRename(id: "abc", name: "Checkout retries")
        let json = try JSONSerialization.jsonObject(
            with: JSONEncoder().encode(body)) as? [String: Any]
        XCTAssertEqual(json?["name"] as? String, "Checkout retries")
        XCTAssertEqual(json?["id"] as? String, "abc")
        XCTAssertNil(json?["title"], "the daemon has no `title` field and rejects the whole body")
    }
}

/// What the person sees about the element they picked.
final class SelectionTests: XCTestCase {
    private func picked(_ json: String) -> Picked {
        try! JSONDecoder().decode(Picked.self, from: Data(json.utf8))
    }

    /// The 23 computed properties were collected for the agent and shown to nobody. The summary
    /// is the one line that says what you are about to change.
    func testTheSelectionSaysWhatItIs() {
        let p = picked(#"""
        {"selector":"button","tag":"button","text":"Sign up","html":"<button/>",
         "style":{"font-size":"14px","font-weight":"600","color":"rgb(255, 255, 255)",
                  "background-color":"rgb(59, 130, 246)"},
         "hints":[],"rect":{"x":0,"y":0,"width":120,"height":32}}
        """#)
        XCTAssertEqual(p.summary, "button · 120×32 · 14px/600 · #ffffff · on #3b82f6")
    }

    /// A transparent background is not a colour anyone wants read out to them.
    func testATransparentBackgroundIsNotMentioned() {
        let p = picked(#"""
        {"selector":"p","tag":"p","text":"","html":"<p/>",
         "style":{"background-color":"rgba(0, 0, 0, 0)"},"hints":[],
         "rect":{"x":0,"y":0,"width":10,"height":4}}
        """#)
        XCTAssertEqual(p.summary, "p · 10×4")
    }

    /// An ambiguous selector is said out loud rather than sent as if it were exact — the after
    /// photo would be of whichever element happened to come first.
    func testAnAmbiguousSelectorIsDeclaredToTheAgent() {
        let ambiguous = picked(#"""
        {"selector":"div > button","tag":"button","text":"","html":"<button/>","style":{},
         "hints":[],"rect":{"x":0,"y":0,"width":1,"height":1},"unique":false}
        """#)
        XCTAssertTrue(ambiguous.describe().contains("matches more than one element"))

        let exact = picked(#"""
        {"selector":"#go","tag":"button","text":"","html":"<button/>","style":{},
         "hints":[],"rect":{"x":0,"y":0,"width":1,"height":1},"unique":true}
        """#)
        XCTAssertFalse(exact.describe().contains("matches more than one element"))
    }
}

/// The check itself, over the pins a turn was actually sent with.
@MainActor
final class DesignCheckTests: XCTestCase {

    private func image(_ color: NSColor) -> NSImage {
        let img = NSImage(size: NSSize(width: 8, height: 8))
        img.lockFocus()
        color.setFill()
        NSRect(x: 0, y: 0, width: 8, height: 8).fill()
        img.unlockFocus()
        return img
    }

    private func picked(_ selector: String) -> Picked {
        let json = """
        {"selector":"\(selector)","tag":"button","text":"","html":"<button/>","style":{},
         "hints":[],"rect":{"x":1,"y":2,"width":30,"height":10}}
        """
        return try! JSONDecoder().decode(Picked.self, from: Data(json.utf8))
    }

    private func model(rects: [String: Picked.Rect],
                       shots: @escaping (Picked.Rect) -> NSImage?) -> SessionModel {
        let m = SessionModel(client: Client(port: 0))
        m.rectsNow = { sels in rects.filter { sels.contains($0.key) } }
        m.resnapshot = { rect in shots(rect) }
        return m
    }

    /// Every pin is compared. It used to take the first and silently drop the rest, so a second
    /// pin paid for a before-image nobody ever looked at.
    func testEveryPinGetsAVerdict() async {
        let red = image(.red)
        let m = model(rects: ["#a": .init(x: 0, y: 0, width: 30, height: 10),
                              "#b": .init(x: 0, y: 40, width: 30, height: 10)],
                      shots: { $0.y == 0 ? self.image(.red) : self.image(.blue) })
        m.designInFlight = [.init(picked: picked("#a"), before: red),
                            .init(picked: picked("#b"), before: red)]
        let turn = Turn(prompt: "make them green")
        await m.checkDesign(turn)
        XCTAssertEqual(turn.design?.pins.count, 2)
        XCTAssertEqual(turn.design?.pins[0].verdict, .nothingChanged)
        XCTAssertEqual(turn.design?.pins[1].verdict, .changed)
    }

    /// The rect is re-read at check time. A pin photographed at its pick-time coordinates after
    /// the page scrolled is a photograph of whatever moved into that square.
    func testTheElementIsRephotographedWhereItIsNow() async {
        var asked: [Picked.Rect] = []
        let m = model(rects: ["#a": .init(x: 5, y: 900, width: 30, height: 10)],
                      shots: { asked.append($0); return self.image(.blue) })
        m.designInFlight = [.init(picked: picked("#a"), before: image(.red))]
        await m.checkDesign(Turn(prompt: "x"))
        // Not the pick-time y of 2.
        XCTAssertEqual(asked.first?.y, 900)
    }

    /// Three different reasons that all used to read "the page was still moving".
    func testNotComparedSaysWhy() async {
        let m = model(rects: [:], shots: { _ in nil })
        m.designInFlight = [.init(picked: picked("#gone"), before: image(.red))]
        let gone = Turn(prompt: "x")
        await m.checkDesign(gone)
        XCTAssertEqual(gone.design?.pins.first?.verdict,
                       .notCompared("the element is no longer on the page"))

        let m2 = model(rects: ["#a": .init(x: 0, y: 0, width: 0, height: 0)], shots: { _ in nil })
        m2.designInFlight = [.init(picked: picked("#a"), before: image(.red))]
        let hidden = Turn(prompt: "x")
        await m2.checkDesign(hidden)
        XCTAssertEqual(hidden.design?.pins.first?.verdict,
                       .notCompared("the element is no longer visible"))

        let m3 = SessionModel(client: Client(port: 0))
        m3.designInFlight = [.init(picked: picked("#a"), before: image(.red))]
        let closed = Turn(prompt: "x")
        await m3.checkDesign(closed)
        guard case .notCompared(let why) = closed.design?.pins.first?.verdict else {
            return XCTFail("a closed pane is not a verdict")
        }
        XCTAssertTrue(why.contains("Designer was closed"), why)
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
        // The shape a real project has, where the app directory is one of several packages.
        XCTAssertEqual(Frontend.route(for: "dashboard/app/reseller-onboarding/page.tsx"),
                       "/reseller-onboarding")
        // A component is not a page. Saying so is what sends `Frontend.page(containing:)` up the
        // import graph instead of leaving the preview where it was.
        XCTAssertNil(Frontend.route(for: "components/reseller-onboarding/domain-step.tsx"))
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
         "hints":[{"kind":"component","value":"Header"}],"rect":{"x":1,"y":2,"width":30,"height":10}}
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

    /// A drag is a sentence, not an edit. Keel never writes the file — it says exactly what was
    /// done by hand, in the units the source uses, and the pixel check afterwards proves the
    /// source now matches.
    func testADragBecomesAnInstruction() {
        let m = SessionModel(client: Client(port: 0))
        var reverted = false
        m.canvas = { if $0["keel"] as? String == "revert" { reverted = true } }
        m.designNudge(picked("#cta", text: "Sign up"), label: "width 240px → 320px")
        m.designNudge(picked("#cta", text: "Sign up"), label: #"text "Sign up" → "Get started""#)
        XCTAssertEqual(m.pins.count, 1, "both changes land on the one element")
        XCTAssertEqual(m.pins[0].nudges.count, 2)

        let p = m.designPrompt("")!
        XCTAssertTrue(p.contains("width 240px → 320px"))
        XCTAssertTrue(p.contains("do not add inline styles"))

        // A dragged pin is a complete request; the box may stay empty.
        m.send()
        XCTAssertTrue(reverted, "the page is put back, so what lands is the agent's change")
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
        // Following the edit is a choice now, off by default; this test is about what happens
        // when it is on.
        m.followEdits = true
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

/// Talking to the daemon.
final class ClientTimeoutTests: XCTestCase {

    /// The session's hour exists for the chat stream, and every ordinary call used to inherit it.
    /// A daemon that never answered `/api/open` left the window dimmed on "Opening …" with
    /// nothing to click and nothing to cancel — for an hour. An ordinary call has to give up.
    func testAnOrdinaryCallDoesNotWaitAnHour() {
        XCTAssertLessThanOrEqual(Client.ordinary, 120,
                                 "long enough for a cold scan, short enough to come back")
        XCTAssertGreaterThan(Client.ordinary, 30, "a cold /api/open on a large repo is slow")
    }

    /// It must actually fail rather than hang: nothing is listening on this port.
    func testACallToADeadDaemonFails() async {
        let client = Client(port: 9)
        do {
            _ = try await client.get("/api/state", as: Wire.State.self)
            XCTFail("a call to a port with no daemon must not succeed")
        } catch {
            // Refused or timed out — either is an answer, which is the whole point.
        }
    }
}
