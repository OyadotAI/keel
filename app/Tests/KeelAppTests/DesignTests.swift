import AppKit
import WebKit
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
        let canvas = FakeCanvas()
        canvas.rects = rects
        canvas.shots = shots
        m.attach(canvas: canvas)
        // Kept alive for the model, which holds it weakly the way it holds the real pane.
        canvases.append(canvas)
        return m
    }
    private var canvases: [FakeCanvas] = []

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

/// The JSON viewer in the preview, run in a real web view against a real JSON document.
///
/// Asserting on the script's text would prove nothing: the only question is whether WebKit runs it
/// on the document it is meant to and leaves every other document alone.
@MainActor
final class JSONViewTests: XCTestCase {

    /// Load `body` as `mime` with the viewer installed, and answer one expression about the result.
    private func inspect(_ body: String, mime: String, _ script: String) throws -> String {
        let source = try XCTUnwrap(Resources.text("JSONView", "js"), "JSONView.js is missing")
        let config = WKWebViewConfiguration()
        config.userContentController.addUserScript(
            WKUserScript(source: source, injectionTime: .atDocumentEnd, forMainFrameOnly: false))
        let web = WKWebView(frame: .init(x: 0, y: 0, width: 400, height: 400), configuration: config)

        let loaded = expectation(description: "loaded")
        let delegate = Loaded { loaded.fulfill() }
        web.navigationDelegate = delegate
        web.load(Data(body.utf8), mimeType: mime, characterEncodingName: "utf-8",
                 baseURL: URL(string: "http://localhost:3000/api/todos")!)
        wait(for: [loaded], timeout: 10)

        var answer = ""
        let evaluated = expectation(description: "evaluated")
        web.evaluateJavaScript(script) { value, _ in
            answer = value.map { String(describing: $0) } ?? ""
            evaluated.fulfill()
        }
        wait(for: [evaluated], timeout: 10)
        return answer
    }

    private final class Loaded: NSObject, WKNavigationDelegate {
        let done: () -> Void
        init(done: @escaping () -> Void) { self.done = done }
        func webView(_ web: WKWebView, didFinish navigation: WKNavigation!) { done() }
    }

    /// The one it is for: an API response is a tree with its keys and values apart, not one
    /// unwrapped line of text.
    func testAJSONResponseBecomesATree() throws {
        let body = #"{"todos":[{"id":1,"done":false,"title":"ship it"}],"page":{"next":null}}"#
        XCTAssertEqual(try inspect(body, mime: "application/json",
                                   "document.body.hasAttribute('data-keel-json')"), "1")
        XCTAssertEqual(try inspect(body, mime: "application/json",
                                   "document.querySelectorAll('.k-key').length > 4"), "1")
        XCTAssertEqual(try inspect(body, mime: "application/json",
                                   "document.querySelector('.k-str').textContent"), "\"ship it\"")
        // Everything drawn is Keel's, so the design canvas never reports the viewer as a region
        // the agent changed.
        XCTAssertEqual(
            try inspect(body, mime: "application/json",
                        "[...document.body.querySelectorAll('*')].every(e => e.hasAttribute('data-keel'))"),
            "1")
    }

    /// An API that answers `text/plain` is still an API.
    func testAPlainTextJSONBodyIsReadAsJSON() throws {
        XCTAssertEqual(try inspect(#"[1,2,3]"#, mime: "text/plain",
                                   "document.body.hasAttribute('data-keel-json')"), "1")
    }

    /// And the half that matters more: a page is a page. A viewer that rewrote an ordinary
    /// document would have broken the preview it lives in.
    func testAPageIsLeftAlone() throws {
        XCTAssertEqual(try inspect("<h1>Hello</h1>", mime: "text/html",
                                   "document.body.hasAttribute('data-keel-json')"), "0")
        // A 404 served as JSON but written as prose parses as nothing, and is left as it arrived.
        XCTAssertEqual(try inspect("Not Found", mime: "application/json",
                                   "document.body.hasAttribute('data-keel-json')"), "0")
        XCTAssertEqual(try inspect("Not Found", mime: "application/json",
                                   "document.body.textContent.trim()"), "Not Found")
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
        let canvas = FakeCanvas()
        var reverted = false
        canvas.sent = { if $0["keel"] as? String == "revert" { reverted = true } }
        m.attach(canvas: canvas)
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
        let canvas = FakeCanvas()
        canvas.sent = { sent.append($0["keel"] as? String ?? "") }
        m.attach(canvas: canvas)
        m.designer.wantsStage = false
        m.record(Data(#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"w1","name":"Write","input":{"file_path":"/repo/frontend/app/pricing/page.tsx"}}]}}"#.utf8), into: t)
        XCTAssertEqual(m.editing, "/repo/frontend/app/pricing/page.tsx")
        XCTAssertEqual(m.previewURL, "http://127.0.0.1:3000/pricing", "navigated to the page")
        XCTAssertEqual(sent.last, "expect")
        XCTAssertTrue(m.designer.wantsStage, "the window is asked to show the page")

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

/// Failures Claude Code reports about itself — the ones that used to render as prose in the
/// agent's own voice, with nothing to press.
@MainActor
final class TroubleTests: XCTestCase {

    private func record(_ json: String) -> SessionModel.Record {
        try! JSONDecoder().decode(SessionModel.Record.self, from: Data(json.utf8))
    }

    /// The real bytes, captured from a `claude -p` run with broken credentials. Not hand-written:
    /// the shape is the CLI's, and a fixture invented from memory would pass while the app failed.
    func testTheRealAuthFailureIsRecognisedAndFixable() throws {
        let url = Bundle.module.url(forResource: "auth-failure", withExtension: "json",
                                   subdirectory: "Fixtures")
        let data = try Data(contentsOf: try XCTUnwrap(url))
        let r = try JSONDecoder().decode(SessionModel.Record.self, from: data)

        let trouble = try XCTUnwrap(SessionModel.classify(r), "an auth failure must be recognised")
        XCTAssertEqual(trouble.kind, "authentication_failed")
        XCTAssertEqual(trouble.message, "Not logged in · Please run /login")
        let fix = try XCTUnwrap(trouble.fix(SessionModel(client: Client(port: 0))),
                                "a failure the person can fix must carry the fix")
        XCTAssertEqual(fix.label, "Log in")
    }

    /// The transcripts Keel replays spell the flag differently from the live stream. A classifier
    /// that knows one works live and fails on every replayed session.
    func testBothSpellingsOfTheFlagAreRead() {
        let live = record(#"""
        {"type":"assistant","error":"server_error","is_api_error_message":true,
         "message":{"model":"<synthetic>","content":[{"type":"text","text":"API Error: 529"}]}}
        """#)
        let replayed = record(#"""
        {"type":"assistant","error":"server_error","isApiErrorMessage":true,
         "message":{"model":"<synthetic>","content":[{"type":"text","text":"API Error: 529"}]}}
        """#)
        XCTAssertEqual(SessionModel.classify(live)?.kind, "server_error")
        XCTAssertEqual(SessionModel.classify(replayed)?.kind, "server_error")
    }

    /// The most common real failure in the corpus. It carries the reset time, and saying "resets
    /// at 7pm" is the difference between waiting and giving up.
    func testARateLimitSaysWhenItEnds() throws {
        let r = record(#"""
        {"type":"assistant","error":"rate_limit","is_api_error_message":true,
         "quotaLimits":{"resetsAt":1788044400,"rateLimitType":"five_hour"},
         "message":{"model":"<synthetic>","content":[{"type":"text","text":"You've hit your session limit"}]}}
        """#)
        let trouble = try XCTUnwrap(SessionModel.classify(r))
        XCTAssertEqual(trouble.kind, "rate_limit")
        XCTAssertTrue(trouble.message.contains("Work can resume"), trouble.message)
    }

    /// Ordinary prose from the agent must not be mistaken for a failure.
    func testARealAssistantMessageIsNotAFailure() {
        let r = record(#"""
        {"type":"assistant","message":{"model":"claude-opus-5",
         "content":[{"type":"text","text":"I have updated the file."}]}}
        """#)
        XCTAssertNil(SessionModel.classify(r))
    }
}

/// The guarantee: nothing the agent emits is dropped.
@MainActor
final class NothingIsDroppedTests: XCTestCase {

    /// A line Keel cannot decode used to return with no log, no counter and nothing on screen —
    /// so a shape the CLI changed would empty the UI with no symptom at all.
    ///
    /// Keeping the line is `record`'s own job now, not the caller's. It has to be, because only
    /// `record` knows whether a line is worth the raw log's 2,000 slots: the stream loop kept
    /// every line it received, and with partial messages that is one line per token, so the log
    /// that exists to guarantee nothing is hidden filled with deltas.
    func testAnUndecodableLineIsCountedNotDiscarded() {
        let m = SessionModel(client: Client(port: 0))
        let t = Turn(prompt: "x")
        m.record(Data("{ this is not json".utf8), into: t)
        XCTAssertEqual(t.unreadable, 1)
        XCTAssertEqual(t.raw.count, 1, "the bytes are still there to look at")
        XCTAssertEqual(t.raw.first?.text, "{ this is not json")
    }

    /// An unfamiliar record type is kept and named rather than silently dropped.
    func testAnUnknownTypeIsKept() {
        let m = SessionModel(client: Client(port: 0))
        let t = Turn(prompt: "x")
        m.record(Data(#"{"type":"something_new_from_the_cli"}"#.utf8), into: t)
        XCTAssertTrue(t.unknown.contains("something_new_from_the_cli"))
    }

    /// Compaction halves the context and used to say nothing at all.
    func testCompactionIsVisible() {
        let m = SessionModel(client: Client(port: 0))
        let t = Turn(prompt: "x")
        m.record(Data(#"{"type":"system","subtype":"compact_boundary"}"#.utf8), into: t)
        XCTAssertTrue(t.raw.contains { $0.text.contains("compacted") })
    }

    /// `turns` is never trimmed, so a long session must not grow without bound.
    func testTheRawBufferIsCapped() {
        let t = Turn(prompt: "x")
        for i in 0..<(Turn.rawCap + 50) { t.note(raw: "line \(i)") }
        XCTAssertEqual(t.raw.count, Turn.rawCap)
        XCTAssertEqual(t.rawDropped, 50, "and it says how many it did not keep")
    }

    /// A folder that is not a repository is the usual cause of "the isolated checkout could not be
    /// created", and Keel can fix it in one click rather than leaving a wall.
    func testANonRepoFailureCarriesTheInitFix() {
        let m = SessionModel(client: Client(port: 0))
        m.isRepo = false
        XCTAssertEqual(m.repoFix?.label, "Initialise git")
        m.isRepo = true
        XCTAssertNil(m.repoFix)
    }
}

/// What leaves the machine when a failure is reported.
@MainActor
final class RedactionTests: XCTestCase {

    /// Every failure the person sees is now reported, which is only acceptable because the message
    /// is stripped first. The rule the repository has always had: no call site passes a prompt, a
    /// path or a repository name.
    func testAPathNeverLeaves() {
        let out = Telemetry.redact(
            "could not open /Users/someone/Projects/secret-client/app: no such file")
        XCTAssertFalse(out.contains("someone"), out)
        XCTAssertFalse(out.contains("secret-client"), out)
        XCTAssertTrue(out.contains("<path>"), out)
        // The part that identifies the bug survives.
        XCTAssertTrue(out.contains("no such file"), out)
    }

    /// A git error quotes the branch, which is usually the feature someone is building.
    func testAQuotedNameNeverLeaves() {
        let out = Telemetry.redact("fatal: a branch named 'keel/acme-billing-migration' already exists")
        XCTAssertFalse(out.contains("acme"), out)
        XCTAssertTrue(out.contains("already exists"), out)
    }

    func testAURLNeverLeaves() {
        let out = Telemetry.redact("failed to reach https://internal.acme.example/api/v2")
        XCTAssertFalse(out.contains("acme"), out)
        XCTAssertTrue(out.contains("<url>"), out)
    }

    /// Long enough to be useful, short enough not to smuggle a transcript out in an error string.
    func testTheReportIsBounded() {
        XCTAssertLessThanOrEqual(Telemetry.redact(String(repeating: "x", count: 5000)).count, 300)
    }
}

/// Numbers parsed out of a page must not be able to crash the app.
///
/// `Int(_: Double)` traps outside `Int`'s range — SIGTRAP, not an exception — and every number in
/// a CSS colour came from `getComputedStyle` on somebody else's page.
@MainActor
final class ColourParsingTests: XCTestCase {
    func testAnOrdinaryColourStillReadsAsHex() {
        XCTAssertEqual(Picked.short("rgb(59, 130, 246)"), "#3b82f6")
    }

    /// `-5` arrives as `5`: the splitter breaks on any non-digit, so the minus is a separator
    /// rather than a sign. That is pre-existing and harmless for a computed colour, which has no
    /// negative channels — it is asserted here so the next person does not read it as the clamp.
    func testAnAbsurdNumberIsClampedRatherThanFatal() {
        XCTAssertEqual(Picked.short("rgb(99999999999999999999999999, -5, 300)"), "#ff05ff")
    }

    func testSomethingThatIsNotAColourComesBackUnchanged() {
        XCTAssertEqual(Picked.short("inherit"), "inherit")
    }
}

/// A page that answers with what the test put in it.
@MainActor
final class FakeCanvas: PreviewCanvas {
    var rects: [String: Picked.Rect] = [:]
    var shots: (Picked.Rect) -> NSImage? = { _ in nil }
    var sent: ([String: Any]) -> Void = { _ in }
    var reloads = 0

    func snapshot(_ rect: Picked.Rect) async -> NSImage? { shots(rect) }
    func rects(for selectors: [String]) async -> [String: Picked.Rect] {
        rects.filter { selectors.contains($0.key) }
    }
    func send(_ message: [String: Any]) { sent(message) }
    func reload() { reloads += 1 }
}
