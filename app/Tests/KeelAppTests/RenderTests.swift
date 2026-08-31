import AppKit
import SwiftUI
import XCTest
@testable import KeelApp

/// Every pane, laid out in the geometry that crashed: zero, one pixel, and a sliver.
///
/// The Designer crashed on the way in because a zero-width proposal made a scale of zero and a
/// height of infinity. That is not a bug a unit test of the model finds; it is one a view has to
/// be *rendered* to find. So each pane is hosted and laid out at the sizes SwiftUI actually
/// proposes during an animated switch. A trap here fails the build, which is the point.
@MainActor
final class RenderTests: XCTestCase {
    private func model() -> SessionModel {
        let m = SessionModel(client: Client(port: 0))
        m.loaded = true
        m.previewURL = "http://127.0.0.1:1/"
        m.changes = [Wire.Change(path: "a/b.txt", status: " M", label: "modified")]
        let t = Turn(prompt: "hello")
        t.finished = true
        t.gate = .passed("make check", 1)
        m.turns = [t]
        return m
    }

    private func layout<V: View>(_ view: V) {
        let host = NSHostingView(rootView: view)
        for size in [CGSize.zero, CGSize(width: 1, height: 1), CGSize(width: 2, height: 400),
                     CGSize(width: 900, height: 600)] {
            host.frame = CGRect(origin: .zero, size: size)
            host.layoutSubtreeIfNeeded()
            host.layout()
        }
    }

    func testEveryPaneSurvivesTinyGeometry() {
        let m = model()
        let lanes = Lanes(client: Client(port: 0), port: 0)
        layout(PreviewSurface(model: m))
        layout(TurnStage(model: m))
        layout(ChatRail(model: m))
        for p in SessionWindow.Panel.allCases {
            layout(SidePanel(panel: p, model: m))
        }
        layout(LaneTabs(lanes: lanes))
        layout(DiffSurface(model: m, path: "a/b.txt"))
    }

    /// The cap a pasted prompt is drawn through.
    ///
    /// Reported as a 2,000 ms App Hang on 0.2.53: `TASCIIEncoder::Encode` under
    /// `LazyStack.measureEstimates`, because a lazy stack estimates every row and `lineLimit` caps
    /// the drawn height without capping what CoreText encodes.
    ///
    /// This checks the helper, not the hang. A timing budget was written first and deleted:
    /// `NSHostingView` laid out offscreen never enters `measureEstimates`, so it passed at 0.09s
    /// with the cap removed. A budget that cannot fail reads as proof and is worse than none.
    /// What is still not covered is a *third* pane drawing `turn.prompt` and forgetting `capped`.
    func testALongPasteIsCappedBeforeItIsDrawn() {
        let paste = String(repeating: "sentry stack frame ", count: 20_000)
        XCTAssertEqual(paste.capped(800).count, 801, "capped to the limit, plus the ellipsis")
        XCTAssertTrue(paste.capped(800).hasSuffix("…"))
        XCTAssertEqual("short".capped(800), "short", "under the limit is left alone")
        XCTAssertEqual("exactly".capped(7), "exactly", "the limit itself is not truncated")
    }
}
