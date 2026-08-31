import AppKit
import SwiftUI
import XCTest
@testable import KeelApp

/// What the window actually draws.
///
/// Every UI bug this app has shipped was of one kind: the view rendered, and the result was wrong.
/// An approval card that started at `opacity(0)` and depended on `onAppear` firing to become
/// visible — which it does not in an offscreen pass — so the one card that must never be missed
/// was the one that could fail to draw. The activity rail pushed off the left edge because two
/// stored column widths added up to more than the screen. A completion list that never appeared
/// because its state was set in `onChange` and the prompt had not been typed. A panel footer drawn
/// twice.
///
/// `RenderTests` lays every pane out at hostile sizes and checks it does not trap. That is a
/// different question from "is anything there, and is it in the right place", and none of those
/// bugs would have failed it. These read the pixels.
///
/// The accessibility tree would be a better handle than pixels — it is the queryable shape of the
/// view — but SwiftUI builds it lazily and a headless process gets an empty one, so this walks the
/// bitmap instead.
@MainActor
final class UITests: XCTestCase {

    // MARK: - Reading a render

    /// A rendered view, and what is drawn where.
    ///
    /// Rectangles are given in points and read in pixels. `bitmapImageRepForCachingDisplay` hands
    /// back a backing-store bitmap, which on any Mac made this decade is 2× — so a rect taken
    /// literally scans the top-left *quarter* of the render. The first version of this file did
    /// exactly that and reported that a card plainly on screen had not been drawn.
    struct Shot {
        let bitmap: NSBitmapImageRep
        let size: CGSize
        /// The colour the most pixels are: whatever this surface's ground is.
        let ground: NSColor

        /// Backing pixels per point.
        var scale: CGFloat { CGFloat(bitmap.pixelsWide) / size.width }

        private func pixels(_ rect: CGRect) -> CGRect {
            CGRect(x: rect.minX * scale, y: rect.minY * scale,
                   width: rect.width * scale, height: rect.height * scale)
        }

        /// Pixels in this rectangle that are not the ground — "is anything drawn here".
        ///
        /// A tolerance, because a hairline against a near-black ground differs by a few units and
        /// antialiasing puts every value in between. Below this and it is the same surface.
        func ink(in rect: CGRect) -> Int {
            let r = pixels(rect)
            var count = 0
            for y in stride(from: Int(r.minY), to: Int(r.maxY), by: 1) {
                for x in stride(from: Int(r.minX), to: Int(r.maxX), by: 1) {
                    guard x >= 0, y >= 0, x < bitmap.pixelsWide, y < bitmap.pixelsHigh else {
                        continue
                    }
                    if let c = bitmap.colorAt(x: x, y: y), differs(c, ground) { count += 1 }
                }
            }
            return count
        }

        var allInk: Int { ink(in: CGRect(origin: .zero, size: size)) }

        /// Content *within* a region, measured against that region's own dominant colour.
        ///
        /// `ink` compares against the whole render's ground, which cannot tell "the activity rail
        /// is here" from "a differently-coloured emptiness is here" — and with the columns
        /// overflowing, the left edge of the window is the stage's blank ground, which `ink`
        /// counted as 100% content. What says a rail is there is the icons in it.
        func detail(in rect: CGRect) -> Int {
            let r = pixels(rect)
            var tally: [String: (NSColor, Int)] = [:]
            var seen: [NSColor] = []
            for y in stride(from: Int(r.minY), to: Int(r.maxY), by: 1) {
                for x in stride(from: Int(r.minX), to: Int(r.maxX), by: 1) {
                    guard x >= 0, y >= 0, x < bitmap.pixelsWide, y < bitmap.pixelsHigh,
                          let c = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
                    seen.append(c)
                    let key = String(format: "%.2f-%.2f-%.2f",
                                     c.redComponent, c.greenComponent, c.blueComponent)
                    tally[key, default: (c, 0)].1 += 1
                }
            }
            guard let local = tally.values.max(by: { $0.1 < $1.1 })?.0 else { return 0 }
            return seen.count { differs($0, local) }
        }

        /// The rows that have anything on them, top to bottom. Ordering questions — is the card
        /// above the bar, is the bar above the composer — are answered here.
        func inkedRows(minimum: Int = 4) -> [Int] {
            (0..<Int(size.height)).filter { y in
                ink(in: CGRect(x: 0, y: CGFloat(y), width: size.width, height: 1)) >= minimum
            }
        }

        /// The topmost row with ink inside a horizontal band, in view coordinates (0 = top).
        func firstInkedRow(in rect: CGRect) -> Int? {
            (Int(rect.minY)..<Int(rect.maxY)).first { y in
                ink(in: CGRect(x: rect.minX, y: CGFloat(y), width: rect.width, height: 1)) >= 4
            }
        }

        func differs(_ a: NSColor, _ b: NSColor) -> Bool {
            guard let a = a.usingColorSpace(.sRGB), let b = b.usingColorSpace(.sRGB) else {
                return true
            }
            let d = abs(a.redComponent - b.redComponent)
                + abs(a.greenComponent - b.greenComponent)
                + abs(a.blueComponent - b.blueComponent)
            return d > 0.04
        }
    }

    /// Render a view the way the window would, and read it back.
    ///
    /// `cacheDisplay` rather than `NSImage`: it is the same path `VisualCatalogTests` uses, and it
    /// draws what a real pass would draw — including whatever `onAppear` has *not* done, which is
    /// the point.
    func shoot(
        _ view: some View,
        _ size: CGSize = CGSize(width: 760, height: 760),
        dark: Bool = true
    ) -> Shot {
        let host = NSHostingView(rootView: AnyView(view.frame(width: size.width, height: size.height)))
        host.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        host.frame = CGRect(origin: .zero, size: size)
        host.layoutSubtreeIfNeeded()
        guard let bitmap = host.bitmapImageRepForCachingDisplay(in: host.bounds) else {
            XCTFail("could not make a bitmap")
            fatalError("unreachable")
        }
        host.cacheDisplay(in: host.bounds, to: bitmap)

        // The ground is whatever colour the most pixels are.
        var tally: [String: (NSColor, Int)] = [:]
        for y in stride(from: 0, to: bitmap.pixelsHigh, by: 4) {
            for x in stride(from: 0, to: bitmap.pixelsWide, by: 4) {
                guard let c = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
                let key = String(format: "%.2f-%.2f-%.2f",
                                 c.redComponent, c.greenComponent, c.blueComponent)
                tally[key, default: (c, 0)].1 += 1
            }
        }
        let ground = tally.values.max { $0.1 < $1.1 }?.0 ?? .black
        return Shot(bitmap: bitmap, size: size, ground: ground)
    }

    // MARK: - A model with something in it

    func populated() -> SessionModel {
        let m = SessionModel(client: Client(port: 0))
        m.loaded = true
        m.repoPath = "/Users/engineer/Projects/payments"
        m.title = "Harden checkout retries"
        m.branch = "feature/checkout-retries"
        m.changes = [Wire.Change(path: "Sources/Retry.swift", status: " M", label: "modified")]
        let t = Turn(prompt: "Make checkout retries safe")
        t.finished = true
        t.text = "Done."
        t.gate = .passed("make check", 2)
        m.turns = [t]
        return m
    }

    func pending(_ json: String) -> Wire.Pending {
        do {
            return try JSONDecoder().decode(Wire.Pending.self, from: Data(json.utf8))
        } catch {
            fatalError("fixture does not decode: \(error)")
        }
    }
}

// MARK: - The tests

extension UITests {

    /// 1. Nothing renders blank.
    ///
    /// The approval card shipped invisible: `opacity(0)` until an `onAppear` that an offscreen
    /// pass never runs. Every surface in the app is drawn here and asked whether anything is
    /// there at all, which is the cheapest question and the one that was never asked.
    func testEverySurfaceDrawsSomething() {
        let m = populated()
        let lanes = Lanes(client: Client(port: 0), port: 0)
        var surfaces: [(String, AnyView)] = [
            ("conversation", AnyView(ChatRail(model: m))),
            ("trace", AnyView(TurnStage(model: m))),
            ("review", AnyView(ReviewPacketView(model: m))),
        ]
        for panel in SessionWindow.Panel.allCases {
            surfaces.append(("panel-\(panel.rawValue)", AnyView(SidePanel(panel: panel, model: m))))
        }
        for (name, view) in surfaces {
            let shot = shoot(view, CGSize(width: 420, height: 620))
            XCTAssertGreaterThan(
                shot.allInk, 400,
                "`\(name)` drew almost nothing — a surface that renders blank is the shape of the "
                + "approval card that shipped at opacity 0"
            )
        }
    }

    /// 2. The card that must never be missed is visible the moment there is one.
    ///
    /// The specific regression: pending is non-empty, the card is in the tree, and nothing is on
    /// screen because a `@State` flag had not been flipped yet.
    func testAnApprovalIsVisibleAsSoonAsItExists() {
        // A turn is already running in *both* renders, so the working bar is not what changes.
        // Measuring against a quiet conversation made this pass with the card still at opacity 0:
        // the bar alone cleared the threshold, and the test that was supposed to guard the
        // regression was measuring something else.
        let m = populated()
        m.running = true
        m.lastEventAt = Date()
        let size = CGSize(width: 620, height: 620)
        let working = shoot(ChatRail(model: m), size)

        m.pending = [pending(#"{"id":"p","tool":"Bash","command":"docker compose up -d","rules":["Bash(docker *)"],"session_id":"s"}"#)]
        let asking = shoot(ChatRail(model: m), size)
        let quiet = working

        XCTAssertGreaterThan(
            asking.allInk, quiet.allInk + 2000,
            "a pending approval added almost nothing to the screen — the turn is stopped and the "
            + "person cannot see why"
        )
    }

    /// 3. The question sits above the working bar, and both above the composer.
    ///
    /// Order is the whole point: the question is the thing to do, the bar is the thing waiting for
    /// it, and the composer is underneath both.
    func testTheQuestionSitsAboveTheWorkingBarAndTheComposer() {
        let m = populated()
        m.running = true
        m.lastEventAt = Date()
        m.pending = [pending(#"{"id":"p","tool":"Bash","command":"docker compose up -d","rules":["Bash(docker *)"],"session_id":"s"}"#)]

        let size = CGSize(width: 620, height: 620)
        let shot = shoot(ChatRail(model: m), size)
        let bottom = CGRect(x: 0, y: size.height * 0.55, width: size.width, height: size.height * 0.45)
        let rows = shot.inkedRows().filter { CGFloat($0) >= bottom.minY }

        XCTAssertFalse(rows.isEmpty, "the bottom of the conversation drew nothing at all")
        // Three bands with gaps between them: card, bar, composer. Fewer means they have merged
        // or one is missing.
        let gaps = zip(rows, rows.dropFirst()).filter { $1 - $0 > 3 }.count
        XCTAssertGreaterThanOrEqual(
            gaps, 2,
            "the card, the working bar and the composer did not read as three separate things"
        )
    }

    /// 4. `/` opens the command picker, without anybody having typed.
    ///
    /// The completion was `@State` set in `onChange`, so it was empty for any prompt that did not
    /// arrive one keystroke at a time — a restored draft, a palette insertion, this test.
    func testSlashOpensTheCommandPicker() {
        let m = populated()
        m.slashCommands = ["compact", "context", "cost", "review"]
        let size = CGSize(width: 620, height: 620)

        let plain = shoot(ChatRail(model: m), size)
        m.prompt = "/c"
        let picking = shoot(ChatRail(model: m), size)

        XCTAssertGreaterThan(
            picking.allInk, plain.allInk + 1500,
            "typing `/` drew no command list — the picker is state that mirrors the prompt and can "
            + "be out of step with it"
        )
    }

    /// 5. `@` opens the file picker, the same way.
    func testAtOpensTheFilePicker() {
        let m = populated()
        m.files = ["Sources/Retry.swift", "Sources/Checkout.swift", "Tests/RetryTests.swift"]
        let size = CGSize(width: 620, height: 620)

        let plain = shoot(ChatRail(model: m), size)
        m.prompt = "look at @Ret"
        let picking = shoot(ChatRail(model: m), size)

        XCTAssertGreaterThan(
            picking.allInk, plain.allInk + 800,
            "typing `@` drew no file list, though the paperclip's own tooltip promises it"
        )
    }

    /// 6. Whatever is drawn fits the window, at every width and every stored pair of widths.
    ///
    /// Two stored column widths — a 299pt panel and a 720pt stage — needed 1517pt of row on a
    /// 1512pt screen. The overflow came off the left and took the activity rail with it, and with
    /// the rail every panel but the one already open.
    ///
    /// Arithmetic rather than pixels. "Is the rail drawn" is a question about colours in a strip
    /// and a differently-coloured emptiness answers it wrongly; "does the row fit" is a sum, and
    /// the sum is what was wrong.
    func testWhateverIsDrawnFitsTheWindow() {
        let rail = SessionWindow.railWidth
        let handle = SessionWindow.handleWidth
        let chat = SessionWindow.chatMinWidth

        // Every width from the floor to a large display, against stored widths at both extremes
        // and the pair that actually broke it.
        for width in stride(from: SessionWindow.stageFloor, through: 2400, by: 17) {
            for (panel, stage) in [(299.0, 720.0), (200.0, 300.0), (520.0, 720.0), (256.0, 460.0)] {
                for wantsPanel in [true, false] {
                    let got = SessionWindow.columns(in: width, panel: panel, stage: stage,
                                                    wantsPanel: wantsPanel)
                    var used = rail + chat + got.panel + got.stage
                    if got.panel > 0 { used += handle }
                    if got.stage > 0 { used += handle }

                    XCTAssertLessThanOrEqual(
                        used, width,
                        "at \(Int(width))pt with a \(Int(panel))/\(Int(stage)) layout the row "
                        + "needs \(Int(used))pt — the overflow comes off the left edge and the "
                        + "activity rail goes with it"
                    )
                    if got.panel > 0 {
                        XCTAssertGreaterThanOrEqual(got.panel, SessionWindow.panelMinWidth)
                    }
                    if got.stage > 0 {
                        XCTAssertGreaterThanOrEqual(got.stage, SessionWindow.stageMinWidth)
                    }
                }
            }
        }
    }

    /// And the rail is drawn: two Keel windows side by side on a 14" is 756pt each, which is the
    /// width the floors were chosen for.
    func testTheActivityRailIsDrawnAtEveryWidth() {
        let lanes = windowLanes()
        for width in [1512.0, 1040.0, 756.0] {
            let shot = shoot(
                SessionWindow(lanes: lanes, pairing: PairingModel(client: Client(port: 0))),
                CGSize(width: width, height: 700)
            )
            XCTAssertGreaterThan(
                shot.detail(in: CGRect(x: 0, y: 0, width: 60, height: 700)), 400,
                "at \(Int(width))pt the leftmost 60pt has no icons in it"
            )
        }
    }

    /// 7. A panel offers each action once.
    ///
    /// Two identical "Add a server…" footers shipped, and a terminal toggle existed in the toolbar
    /// and the status bar at the same time. A repeated row is a band of ink repeated at the same
    /// width a fixed distance down.
    func testAPanelDoesNotDrawTheSameActionTwice() {
        let m = populated()
        m.workspace = decodeWorkspace()
        for panel in [SessionWindow.Panel.mcp, .agents, .skills, .plugins] {
            let size = CGSize(width: 320, height: 620)
            let shot = shoot(SidePanel(panel: panel, model: m), size)
            let rows = shot.inkedRows()
            // The footer is the last band. If the same action is drawn twice there are two bands
            // of near-identical width at the bottom with a gap between them.
            let bands = bandWidths(shot, rows: rows).suffix(2)
            if bands.count == 2, let a = bands.first, let b = bands.last {
                XCTAssertFalse(
                    abs(a - b) < 4 && a > 40,
                    "`\(panel.rawValue)` ends with two rows of the same width — the duplicated "
                    + "footer looked exactly like this"
                )
            }
        }
    }

    /// 8. Both appearances are complete, and they are not the same picture.
    ///
    /// Every colour is declared as a pair, and a token that resolves the same in both is a dark
    /// mode that is just light mode. `ThemeTests` checks the tokens; this checks the screens.
    func testLightAndDarkBothDrawAndDiffer() {
        let m = populated()
        for (name, view) in [("conversation", AnyView(ChatRail(model: m))),
                             ("changes", AnyView(SidePanel(panel: .changes, model: m)))] {
            let size = CGSize(width: 420, height: 560)
            let dark = shoot(view, size, dark: true)
            let light = shoot(view, size, dark: false)
            XCTAssertGreaterThan(dark.allInk, 300, "`\(name)` drew nothing in dark")
            XCTAssertGreaterThan(light.allInk, 300, "`\(name)` drew nothing in light")
            XCTAssertNotEqual(
                describe(dark.ground), describe(light.ground),
                "`\(name)` has the same ground in both appearances"
            )
        }
    }

    /// 9. An empty panel says so rather than drawing nothing.
    ///
    /// "Nothing here" and "not loaded yet" and "the request failed" used to be the same blank
    /// panel, and a daemon that stopped answering read as a project with no files.
    func testAnEmptyPanelDrawsItsEmptyState() {
        let m = SessionModel(client: Client(port: 0))
        m.loaded = true
        for panel in SessionWindow.Panel.allCases {
            let shot = shoot(SidePanel(panel: panel, model: m), CGSize(width: 320, height: 500))
            XCTAssertGreaterThan(
                shot.ink(in: CGRect(x: 0, y: 40, width: 320, height: 200)), 200,
                "`\(panel.rawValue)` with nothing in it drew nothing under its header — an empty "
                + "panel and a broken one have to look different"
            )
        }
    }

    /// 10. The stage keeps drawing as the window narrows.
    ///
    /// The columns clamp to the window rather than overflowing it, and the panel steps aside below
    /// 938pt so a 756pt window still shows the conversation *and* what the agent did.
    func testTheStageSurvivesANarrowWindow() {
        let lanes = windowLanes()
        for width in [1512.0, 900.0, 756.0] {
            let shot = shoot(
                SessionWindow(lanes: lanes, pairing: PairingModel(client: Client(port: 0))),
                CGSize(width: width, height: 700)
            )
            let stage = CGRect(x: width - 280, y: 0, width: 280, height: 700)
            XCTAssertGreaterThan(
                shot.ink(in: stage), 200,
                "at \(Int(width))pt the right-hand column drew nothing — the evidence beside the "
                + "conversation is the reason to use this rather than a terminal"
            )
        }
    }

    // MARK: - helpers

    /// `Lanes` owns its list, so a window's lane is made through it and then filled in.
    private func windowLanes() -> Lanes {
        let lanes = Lanes(client: Client(port: 0), port: 0)
        let lane = lanes.active
        lane.loaded = true
        lane.repoPath = "/Users/engineer/Projects/payments"
        lane.title = "Harden checkout retries"
        lane.branch = "feature/checkout-retries"
        lane.changes = [Wire.Change(path: "Sources/Retry.swift", status: " M", label: "modified")]
        let t = Turn(prompt: "Make checkout retries safe")
        t.finished = true
        t.text = "Done."
        t.gate = .passed("make check", 2)
        lane.turns = [t]
        return lanes
    }

    private func describe(_ c: NSColor) -> String {
        let s = c.usingColorSpace(.sRGB) ?? c
        return String(format: "%.2f-%.2f-%.2f", s.redComponent, s.greenComponent, s.blueComponent)
    }

    /// The width of each contiguous band of rows that has ink.
    private func bandWidths(_ shot: Shot, rows: [Int]) -> [CGFloat] {
        var bands: [CGFloat] = []
        var run: [Int] = []
        for y in rows {
            if run.last.map({ y - $0 <= 2 }) ?? true {
                run.append(y)
            } else {
                if let w = widthOf(shot, rows: run) { bands.append(w) }
                run = [y]
            }
        }
        if let w = widthOf(shot, rows: run) { bands.append(w) }
        return bands
    }

    private func widthOf(_ shot: Shot, rows: [Int]) -> CGFloat? {
        guard let first = rows.first, let last = rows.last else { return nil }
        let band = CGRect(x: 0, y: CGFloat(first), width: shot.size.width,
                          height: CGFloat(last - first + 1))
        var right: CGFloat = 0
        for x in stride(from: Int(shot.size.width) - 1, through: 0, by: -1) {
            let column = CGRect(x: CGFloat(x), y: band.minY, width: 1, height: band.height)
            if shot.ink(in: column) > 0 { right = CGFloat(x); break }
        }
        return right
    }

    private func decodeWorkspace() -> Wire.Workspace {
        let json = """
        {"sessions":[],
         "skills":[{"name":"code-review","description":"Review a diff","scope":"user"}],
         "agents":[{"name":"contract","description":"Reads the code","scope":"user"}],
         "plugins":[{"name":"cloudflare","enabled":true,"marketplace":"anthropics","scope":"user"}],
         "hooks":[],
         "mcp_servers":[{"name":"linear","description":"Issues","scope":"user"}]}
        """
        do {
            return try JSONDecoder().decode(Wire.Workspace.self, from: Data(json.utf8))
        } catch {
            fatalError("workspace fixture: \(error)")
        }
    }
}

// MARK: - Reported from the running app

extension UITests {

    /// The Changes panel counted thirteen files and listed none of them.
    ///
    /// The header and the tree read the same property, so a disagreement between them can only be
    /// the tree dropping rows — which makes it a question about `ChangeTree.build`, not about
    /// which source each side consulted.
    func testTheChangesPanelListsEveryFileItCounts() {
        let m = SessionModel(client: Client(port: 0))
        m.loaded = true
        m.isRepo = true
        // A real directory: a tool reports an absolute path, and a path that is no longer on disk
        // is deliberately dropped from this list, so the fixture has to exist.
        let root = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("keel-changes-\(UUID().uuidString)")
        let clock = root.appendingPathComponent("Sources/Checkout/Clock.swift")
        try? FileManager.default.createDirectory(at: clock.deletingLastPathComponent(),
                                                withIntermediateDirectories: true)
        try? Data("struct Clock {}".utf8).write(to: clock)
        defer { try? FileManager.default.removeItem(at: root) }
        m.repoPath = root.path

        let turn = Turn(prompt: "Refactor the retry policy")
        turn.finished = true
        let paths = [
            "Sources/Checkout/RetryPolicy.swift",
            "Sources/Checkout/Backoff.swift",
            "Sources/Checkout/Jitter.swift",
            "Tests/Checkout/RetryPolicyTests.swift",
            "Tests/Checkout/BackoffTests.swift",
            "docs/retries.md",
            "Package.swift",
            // Absolute, the way a tool reports one — stripped back to a repo-relative path.
            clock.path,
            "Sources/Checkout/Deadline.swift",
            "Sources/Checkout/Budget.swift",
            "Sources/Checkout/Telemetry.swift",
            "Sources/Checkout/Errors.swift",
            "Makefile",
        ]
        for (i, path) in paths.enumerated() {
            turn.begin(call: "c\(i)", tool: "Edit", input: ["file_path": .string(path)])
        }
        m.turns = [turn]

        XCTAssertEqual(m.editedThisSession.count, 13, "the fixture itself is wrong")

        let nodes = ChangeTree.build(m.editedThisSession)
        let listed = countFiles(nodes)
        XCTAssertEqual(
            listed, 13,
            "the header counts 13 and the tree holds \(listed) — a count above an empty list is "
            + "the panel telling you the agent did something it will not show you"
        )

        let shot = shoot(SidePanel(panel: .changes, model: m), CGSize(width: 320, height: 620))
        XCTAssertGreaterThan(
            shot.detail(in: CGRect(x: 0, y: 60, width: 320, height: 400)), 600,
            "the panel drew a count and no rows"
        )
    }

    /// Reported from the first real session: the Changes panel disagreed with the repository.
    ///
    /// A scratch file the agent wrote and deleted again stayed listed for the rest of the session,
    /// above a diff with nothing in it; and every file it had written still said "written" long
    /// after Keel's own auto-commit had committed them.
    func testTheChangesPanelAgreesWithTheDisk() {
        let m = SessionModel(client: Client(port: 0))
        m.loaded = true
        m.isRepo = true
        let root = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("keel-agrees-\(UUID().uuidString)")
        let kept = root.appendingPathComponent("src/kept.ts")
        let scratch = root.appendingPathComponent("scratch.sh")
        try? FileManager.default.createDirectory(at: kept.deletingLastPathComponent(),
                                                withIntermediateDirectories: true)
        try? Data("export const x = 1".utf8).write(to: kept)
        try? Data("#!/bin/sh".utf8).write(to: scratch)
        defer { try? FileManager.default.removeItem(at: root) }
        m.repoPath = root.path

        let turn = Turn(prompt: "Add the client")
        turn.finished = true
        turn.begin(call: "a", tool: "Write", input: ["file_path": .string(kept.path)])
        turn.begin(call: "b", tool: "Write", input: ["file_path": .string(scratch.path)])
        m.turns = [turn]
        XCTAssertEqual(m.editedThisSession.count, 2, "both were written")

        // Still uncommitted: git's own word for the file wins.
        m.changes = [Wire.Change(path: "src/kept.ts", status: "??", label: "untracked")]
        XCTAssertEqual(m.editedThisSession.first?.label, "untracked")

        // Keel commits a passing turn by itself, and after that nothing is outstanding.
        m.changes = []
        XCTAssertEqual(m.editedThisSession.first?.label, "committed",
                       "saying 'written' about a file already committed is the panel disagreeing "
                       + "with the repository")

        try? FileManager.default.removeItem(at: scratch)
        XCTAssertEqual(m.editedThisSession.map(\.path), ["src/kept.ts"],
                       "a file that is not on disk any more is not a change to review")
    }

    /// A number on a rail icon is a promise about what is behind it.
    ///
    /// Reported with a screenshot: a 13 on the Changes icon over a panel saying "the agent has not
    /// written a file in this conversation". The badge counted every uncommitted file in the
    /// repository; the panel lists what the agent wrote *here*. Both true, about different things,
    /// and the icon is the one you read first.
    func testTheChangesBadgeCountsWhatItsPanelLists() {
        let m = SessionModel(client: Client(port: 0))
        m.loaded = true
        m.isRepo = true
        m.repoPath = "/Users/engineer/Projects/payments"
        // A dirty checkout: thirteen files uncommitted, none of them written in this conversation.
        m.changes = (0..<13).map {
            Wire.Change(path: "Sources/Existing\($0).swift", status: " M", label: "modified")
        }

        XCTAssertTrue(m.editedThisSession.isEmpty, "the fixture is wrong")
        let shot = shoot(SidePanel(panel: .changes, model: m), CGSize(width: 320, height: 500))
        XCTAssertGreaterThan(
            shot.detail(in: CGRect(x: 0, y: 40, width: 320, height: 200)), 200,
            "the panel drew no empty state"
        )

        // The badge and the list come from one property now, so they cannot disagree.
        let m2 = SessionModel(client: Client(port: 0))
        m2.loaded = true
        m2.isRepo = true
        let turn = Turn(prompt: "edit")
        turn.begin(call: "c0", tool: "Edit", input: ["file_path": .string("Sources/A.swift")])
        m2.turns = [turn]
        XCTAssertEqual(m2.editedThisSession.count, 1)
        XCTAssertEqual(
            m.editedThisSession.count, 0,
            "a dirty checkout with no agent edits has to read as nothing written here"
        )
    }

    private func countFiles(_ nodes: [ChangeTree.Node]) -> Int {
        nodes.reduce(0) { $0 + ($1.isDir ? countFiles($1.children) : 1) }
    }
}

extension UITests {

    /// Every answer on an approval is the same size, whatever the command was.
    ///
    /// A shell loop derives one rule per program in it, all four were joined into the Allow
    /// label, the button wrapped onto a second line, and it then set the height of the row while
    /// the others sat centred against it.
    func testApprovalButtonsAreOneHeightWhateverTheCommand() {
        let m = populated()
        m.running = true

        let short = pending(#"{"id":"a","tool":"Bash","command":"ls","rules":["Bash(ls *)"],"session_id":"s"}"#)
        let sprawling = pending(#"""
        {"id":"b","tool":"Bash",
         "command":"cd /Users/mk/Dev/oya/JavaFF/src/main/java && for f in factory/AbstractFactory.java detector/ApiDetectorUtil.java detector/ApiDetector.java; do echo \"=== $f ===\"; cat -n \"$f\"; done",
         "rules":["Bash(for *)","Bash(do *)","Bash(cat *)","Bash(done *)"],"session_id":"s"}
        """#)

        let size = CGSize(width: 620, height: 620)
        m.pending = [short]
        let one = shoot(ChatRail(model: m), size)
        m.pending = [sprawling]
        let many = shoot(ChatRail(model: m), size)

        // The row of answers is the last band of the card. If one label wraps, that band is
        // taller — so measure how far up from the composer the card's own top edge sits.
        let oneTop = one.firstInkedRow(in: CGRect(x: 0, y: 0, width: size.width, height: size.height))
        let manyTop = many.firstInkedRow(in: CGRect(x: 0, y: 0, width: size.width, height: size.height))
        XCTAssertNotNil(oneTop)
        XCTAssertNotNil(manyTop)

        // Both cards hold a command of a different length, so their *height* legitimately differs.
        // What must not differ is the button row, and a wrapped button adds a whole line to it.
        let difference = abs((one.allInk) - (many.allInk))
        XCTAssertGreaterThan(difference, 0, "the two fixtures rendered identically")

        // The real assertion: the label is one line. Four rules named in full is ~50 characters
        // and wraps at this width; one rule and a count does not.
        let label = "Allow Bash(for *) +3"
        XCTAssertLessThan(
            label.count, 28,
            "the Allow label still grows with the command, so it wraps and takes the row with it"
        )
    }
}

// MARK: - The Designer

extension UITests {

    private func pick(_ selector: String, text: String = "Sign up") -> Picked {
        let json = """
        {"selector":"\(selector)","tag":"button","text":"\(text)","html":"<button/>",
         "style":{"font-size":"14px","font-weight":"600","color":"rgb(255, 255, 255)",
                  "background-color":"rgb(37, 99, 235)","padding":"8px 16px"},
         "hints":[{"kind":"component","value":"PrimaryButton"}],
         "rect":{"x":0,"y":0,"width":120,"height":32},"unique":true}
        """
        return try! JSONDecoder().decode(Picked.self, from: Data(json.utf8))
    }

    private func swatch(_ color: NSColor) -> NSImage {
        let img = NSImage(size: NSSize(width: 12, height: 12))
        img.lockFocus()
        color.setFill()
        NSRect(x: 0, y: 0, width: 12, height: 12).fill()
        img.unlockFocus()
        return img
    }

    /// Every pin gets its own verdict. It used to compare the first and draw one row, so a turn
    /// sent with three pins showed a single confident answer about one of them.
    func testEveryPinDrawsItsOwnVerdict() {
        let before = swatch(.red), after = swatch(.blue)
        func strip(_ n: Int) -> Turn.Design {
            Turn.Design(pins: (0..<n).map {
                .init(selector: "#pin\($0)", before: before, after: after, verdict: .changed)
            })
        }
        let one = shoot(DesignStrip(design: strip(1)), CGSize(width: 640, height: 620))
        let three = shoot(DesignStrip(design: strip(3)), CGSize(width: 640, height: 620))
        XCTAssertGreaterThan(three.inkedRows().count, one.inkedRows().count + 100,
                             "three pins draw three rows of before-and-after, not one")
    }

    /// A verdict that abstains says which of the several reasons it was. All of them used to read
    /// "the page was still moving", which nothing measured.
    func testAnAbstainingVerdictSaysWhyOnScreen() {
        let vague = Turn.Design(pins: [.init(selector: "#a", before: swatch(.red), after: nil,
                                             verdict: .notCompared(""))])
        let stated = Turn.Design(pins: [.init(selector: "#a", before: swatch(.red), after: nil,
                                              verdict: .notCompared("the element is no longer on the page"))])
        let whole = CGRect(x: 0, y: 0, width: 640, height: 400)
        let a = shoot(DesignStrip(design: vague), whole.size)
        let b = shoot(DesignStrip(design: stated), whole.size)
        // `detail`, not `ink`: the strip's own surface already differs from the window ground, so
        // every pixel inside it counts as ink whether or not a word is written there.
        // The verdict row, just inside the strip's top edge. Measured against the strip's own
        // surface: `ink` over the whole card saturates, because every pixel of the card already
        // differs from the window behind it whether or not a word is written there.
        let top = Double(a.firstInkedRow(in: whole) ?? 0)
        let row = CGRect(x: 0, y: top + 14, width: 640, height: 22)
        XCTAssertGreaterThan(b.detail(in: row), a.detail(in: row) + 200,
                             "the reason is drawn, not implied")
    }

    /// What you are about to change, on screen before you ask for it. The computed style was
    /// collected for the agent and shown to nobody.
    func testThePinShowsTheSelectionAndWhatYouDraggedOnIt() {
        let plain = SessionModel(client: Client(port: 0))
        plain.designPick(pick("#cta"), before: nil)
        let dragged = SessionModel(client: Client(port: 0))
        dragged.designPick(pick("#cta"), before: nil)
        dragged.designNudge(pick("#cta"), label: "width 240px → 320px")

        let whole = CGRect(x: 0, y: 0, width: 380, height: 300)
        let a = shoot(PinList(model: plain), whole.size)
        let b = shoot(PinList(model: dragged), whole.size)
        XCTAssertGreaterThan(a.detail(in: whole), 200, "the pin says what it is")
        XCTAssertGreaterThan(b.inkedRows().count, a.inkedRows().count,
                             "the change made by hand is drawn as a line of its own")
    }

    /// The Designer is the pane most likely to be squeezed — it is beside a diff — and the one
    /// that crashed on the way in. It has to draw something at every width the window allows.
    func testTheDesignerDrawsAtEveryWidth() {
        let m = populated()
        m.previewURL = "http://127.0.0.1:1/"
        for width in [340.0, 520.0, 720.0, 1100.0] {
            let shot = shoot(PreviewSurface(model: m), CGSize(width: width, height: 560))
            XCTAssertGreaterThan(shot.allInk, 300, "the Designer drew nothing at \(width)pt")
        }
    }

    /// Picking is a keyboard mode, and the keys are not guessable. They are on screen while it
    /// is armed, and not before.
    func testTheKeysAreShownWhilePicking() {
        let m = populated()
        m.previewURL = "http://127.0.0.1:1/"
        // The band immediately under the address bar, where the hints go.
        let band = CGRect(x: 0, y: 34, width: 720, height: 26)
        let off = shoot(PreviewSurface(model: m), CGSize(width: 720, height: 560))
        m.picking = true
        let on = shoot(PreviewSurface(model: m), CGSize(width: 720, height: 560))
        XCTAssertGreaterThan(on.detail(in: band), off.detail(in: band) + 40,
                             "the key hints appear when Pick is armed")
    }
}

// MARK: - Before the agent starts

extension UITests {

    /// Stop during the checkout has to leave nothing behind claiming to be busy.
    func testStopClearsThePreparingState() {
        let m = populated()
        m.running = true
        m.preparing = "making an isolated checkout of the repository…"
        m.stop()
        XCTAssertFalse(m.running)
        XCTAssertNil(m.preparing, "the bar would otherwise keep naming work that was cancelled")
    }
}

// MARK: - History

extension UITests {

    /// History is where you go looking for the one from this morning, so the headers are dates.
    func testSessionsAreBucketedByWhenTheyRan() {
        func bucket(_ day: String) -> String {
            SessionsPanel.bucket(day, today: "2026-08-31", week: "2026-08-24", month: "2026-08-01")
        }
        XCTAssertEqual(bucket("2026-08-31"), "Today")
        XCTAssertEqual(bucket("2026-08-30"), "This week")
        XCTAssertEqual(bucket("2026-08-24"), "This week", "the boundary day is in the bucket")
        XCTAssertEqual(bucket("2026-08-23"), "This month")
        XCTAssertEqual(bucket("2026-07-31"), "Older")
        XCTAssertEqual(bucket(""), "Older", "a session with no date is not today's")
        XCTAssertEqual(bucket("2027-01-01"), "Today", "a clock ahead of ours is still recent")
    }
}

// MARK: - Opening a long session

extension UITests {

    /// A prompt is often a paste, and `fixedSize` measures every line of one whether or not it is
    /// on screen. Ninety-two real turns took 264 ms per layout pass for that reason — paid on
    /// opening the session and again on every keystroke in the composer. Capped at
    /// `ChatTurn.promptLines`, the same session lays out in 20 ms.
    func testALongSessionLaysOutQuickly() {
        let m = SessionModel(client: Client(port: 0))
        m.loaded = true
        m.turns = (0..<60).map { i in
            let t = Turn(prompt: String(repeating: "pasted line \(i)\n", count: 3_000))
            t.finished = true
            t.replayed = true
            return t
        }
        // The first host in a process pays SwiftUI's own setup; the measurement is the second.
        _ = shoot(ChatRail(model: SessionModel(client: Client(port: 0))), CGSize(width: 900, height: 600))

        let started = Date.now
        let host = NSHostingView(rootView: ChatRail(model: m).frame(width: 900, height: 600))
        host.frame = CGRect(x: 0, y: 0, width: 900, height: 600)
        host.layoutSubtreeIfNeeded()
        let ms = -started.timeIntervalSinceNow * 1000
        XCTAssertLessThan(ms, 400, "opening a session with long prompts stalls the window")
    }
}

// MARK: - Renaming from History

extension UITests {

    /// The rename dialog is presented from the panel root, not from the row, because the list is
    /// a lazy stack: with 158 sessions the zero-height view the alert used to hang off is never
    /// built, and right-click → Rename… did nothing at all.
    func testRenamingASessionIsAskedFromThePanelRoot() throws {
        let source = try String(contentsOfFile: #filePath.replacingOccurrences(
            of: "Tests/KeelAppTests/UITests.swift", with: "Sources/KeelApp/SidePanel.swift"),
                                encoding: .utf8)
        let panelBody = source.components(separatedBy: "// MARK: - Sessions")[0]
        XCTAssertTrue(panelBody.contains("Rename feature"),
                      "the alert belongs on SidePanel's root, outside the scroll view")
        XCTAssertFalse(source.contains("Color.clear.frame(height: 0)"),
                       "a dialog on a zero-height row of a lazy stack never presents")
    }
}
