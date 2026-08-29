import AppKit
import SwiftUI
import WebKit

/// What you picked in the running app, and where it probably came from.
struct Picked: Decodable {
    var selector: String
    var tag: String
    var text: String
    var html: String
    var style: [String: String]
    var hints: [Hint]
    var rect: Rect
    var dpr: Double

    struct Hint: Decodable, Identifiable, Equatable {
        /// `attribute`, `fiber`, or `component` — in descending order of how much it is worth.
        var kind: String
        var value: String
        var id: String { kind + value }
    }

    struct Rect: Decodable {
        var x: Double, y: Double, width: Double, height: Double
    }

    /// The prompt this becomes.
    ///
    /// The candidates are named in the prompt rather than resolved silently, for the same reason
    /// they are shown on screen: the failure everyone else has here is a confident guess at the
    /// wrong file, and the fix is to make the guess visible rather than to guess harder.
    func prompt(instruction: String) -> String {
        var out = "Change this element in the running app:\n\n"
        out += "selector: \(selector)\n"
        if !text.isEmpty { out += "text: \(text)\n" }
        if !hints.isEmpty {
            out += "\nLikely source, best first — check before editing, and say which you used:\n"
            for h in hints { out += "  - \(h.value)  (\(h.kind))\n" }
            out += "If none of these is right, find the component that renders it and edit that "
                + "one. Do not create a new component: this element already exists somewhere.\n"
        }
        out += "\ncomputed style:\n"
        for (k, v) in style.sorted(by: { $0.key < $1.key }) { out += "  \(k): \(v)\n" }
        out += "\nmarkup:\n\(html)\n\n\(instruction)"
        return out
    }
}

/// The preview, and the picker that turns a click into a prompt.
struct PreviewPane: NSViewRepresentable {
    let url: URL
    let model: SessionModel
    @Binding var picking: Bool

    func makeCoordinator() -> Coordinator { Coordinator(model: model) }

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        // The whole trick: `forMainFrameOnly: false` puts the picker inside the dev server's frame
        // regardless of its origin. A page script cannot reach across that boundary; a user script
        // is installed by the host and does not have to.
        if let js = Bundle.module.url(forResource: "Picker", withExtension: "js"),
           let source = try? String(contentsOf: js, encoding: .utf8) {
            config.userContentController.addUserScript(
                WKUserScript(source: source, injectionTime: .atDocumentEnd, forMainFrameOnly: false))
        }
        config.userContentController.add(context.coordinator, name: "keel")

        let view = WKWebView(frame: .zero, configuration: config)
        view.load(URLRequest(url: url))
        context.coordinator.web = view
        context.coordinator.loaded = url

        // The after-photo has to come from this live web view, so the coordinator that owns it
        // lends the model a way back in. Cleared when the pane goes away, which is what makes an
        // un-photographable comparison report `unstable` instead of quietly passing.
        let coordinator = context.coordinator
        model.resnapshot = { [weak coordinator] picked in
            await coordinator?.snapshot(picked)
        }
        return view
    }

    func updateNSView(_ view: WKWebView, context: Context) {
        context.coordinator.web = view

        // Navigate when the address changes. The URL was loaded once in `makeNSView` and never
        // again, so typing a new one updated the field and nothing else — the pane just kept
        // showing whatever it had first.
        if context.coordinator.loaded != url {
            context.coordinator.loaded = url
            view.load(URLRequest(url: url))
        }

        let message = picking ? "pick-on" : "pick-off"
        // Posted to every frame, since the picker lives in the one the dev server owns.
        view.evaluateJavaScript("""
            (function(){
              window.postMessage({keel:'\(message)'}, '*');
              for (var i=0;i<window.frames.length;i++) {
                try { window.frames[i].postMessage({keel:'\(message)'}, '*'); } catch(e){}
              }
            })();
            """)
    }

    @MainActor
    final class Coordinator: NSObject, WKScriptMessageHandler {
        let model: SessionModel
        weak var web: WKWebView?
        /// What the view was last told to load, so an unchanged address is not reloaded on every
        /// pass of `updateNSView` — which would restart the page under you on every keystroke.
        var loaded: URL?
        init(model: SessionModel) { self.model = model }

        func userContentController(_ c: WKUserContentController, didReceive message: WKScriptMessage) {
            guard let dict = message.body as? [String: Any],
                  let data = try? JSONSerialization.data(withJSONObject: dict),
                  let picked = try? JSONDecoder().decode(Picked.self, from: data) else { return }
            Task { await capture(picked) }
        }

        /// The same rect again, once the page has rebuilt.
        func snapshot(_ picked: Picked) async -> NSImage? {
            guard let web, picked.rect.width > 1, picked.rect.height > 1 else { return nil }
            let config = WKSnapshotConfiguration()
            config.rect = CGRect(x: picked.rect.x, y: picked.rect.y,
                                 width: picked.rect.width, height: picked.rect.height)
            return try? await web.takeSnapshot(configuration: config)
        }

        /// The before image, cropped to the element.
        ///
        /// `takeSnapshot` with a rect is exact and native — no screen recording permission, no
        /// window capture, and it sees the element as the page actually rendered it. This is the
        /// half nobody else has: a UI change gets a pixel diff, not only a text one.
        private func capture(_ picked: Picked) async {
            model.designPick(picked, before: await snapshot(picked))
        }
    }
}

/// The preview tab: an address, the dev server, and the pick toggle.
struct PreviewSurface: View {
    @Bindable var model: SessionModel

    @State private var starting = false
    @State private var typed = ""
    @State private var editing = false

    var body: some View {
        VStack(spacing: 0) {
            bar
            Hairline()
            if let s = model.previewURL, let url = URL(string: s) {
                // The page is rendered at a real width and scaled to fit, rather than squeezed
                // into the pane. The pane is 340–720pt, so a responsive site was correctly
                // rendering its phone layout — and there was no way to ask for anything else.
                GeometryReader { geo in
                    let target = model.previewWidth.points
                    let scale = min(1, (geo.size.width - 2) / target)
                    PreviewPane(url: url, model: model, picking: $model.picking)
                        .frame(width: target, height: geo.size.height / scale)
                        .scaleEffect(scale, anchor: .top)
                        .frame(width: geo.size.width, height: geo.size.height, alignment: .top)
                        .clipped()
                }
                .background(K.C.well)
            } else if starting {
                waiting
            } else {
                empty
            }
        }
        // Opening the preview is asking to see the app. If the project declares how to run one and
        // nothing is running, run it — a tab whose only content is a button saying "run the thing
        // you just asked to see" is a tab that has not done its job.
        .task {
            await model.refreshDev()
            guard model.previewURL == nil, !model.devRunning, model.devDetected != nil,
                  !starting else { return }
            starting = true
            await model.startDev()
            starting = false
        }
    }

    private var waiting: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            HStack(spacing: K.S.sm) {
                Sweep()
                Text("Starting \(model.devDetected ?? "the dev server")…")
                    .font(K.F.small).foregroundStyle(K.C.dim)
            }
            Text("Keel points the preview at whatever URL it announces.")
                .font(K.F.micro).foregroundStyle(K.C.faint)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
    }

    private var bar: some View {
        HStack(spacing: 8) {
            TextField("localhost:3000", text: $typed)
                .textFieldStyle(.plain)
                .font(K.F.code)
                .padding(.horizontal, K.S.sm).padding(.vertical, 4)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
                // Committed on Enter, not on every keystroke. Normalising as you typed rewrote
                // the field under the cursor — the first character became `http://l` and it went
                // downhill from there.
                .onSubmit { go() }
                .onAppear { typed = model.previewURL ?? "" }
                .onChange(of: model.previewURL) { _, new in
                    // Follow the model when something else sets it (a dev server starting, a URL
                    // noticed in output), but never while you are mid-edit.
                    if let new, !editing { typed = new }
                }
                .onChange(of: typed) { editing = true }

            Button("Go") { go() }
                .buttonStyle(QuietButton())
                .disabled(typed.trimmingCharacters(in: .whitespaces).isEmpty)

            // Desktop first: a preview exists to show the app, and the app is a desktop app until
            // someone says otherwise.
            HStack(spacing: 0) {
                ForEach(PreviewWidth.allCases) { w in
                    let on = model.previewWidth == w
                    Image(systemName: w.icon)
                        .font(.system(size: 10))
                        .foregroundStyle(on ? K.C.text : K.C.faint)
                        .frame(width: 24, height: 20)
                        .background(
                            RoundedRectangle(cornerRadius: K.R.sm - 1)
                                .fill(on ? K.C.raised : .clear)
                                .padding(1)
                        )
                        .contentShape(Rectangle())
                        .onTapGesture { model.previewWidth = w }
                        .help("\(w.title) — \(Int(w.points))pt wide")
                }
            }
            .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))

            Toggle(isOn: $model.picking) {
                Label("Pick", systemImage: "cursorarrow.rays")
            }
            .toggleStyle(.button)
            .controlSize(.small)
            .help("Click an element in the page to describe it to the agent")

            if model.devRunning {
                HStack(spacing: 4) {
                    Circle().fill(K.C.add).frame(width: 5, height: 5)
                    Text("running").font(K.F.micro).foregroundStyle(K.C.add)
                }
                Button("Stop") { Task { await model.stopDev() } }.buttonStyle(QuietButton())
            } else if let d = model.devDetected {
                Button("Run \(d)") { Task { await model.startDev() } }
                    .buttonStyle(QuietButton())
            }
            Button {
                Task { await model.refreshDev() }
            } label: {
                Image(systemName: "arrow.clockwise").font(.system(size: 10))
                    .frame(width: 20, height: 18).contentShape(Rectangle())
            }
            .buttonStyle(.plain).foregroundStyle(K.C.faint)
            .help("Reload")
        }
        .padding(8)
    }

    private var empty: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Nothing to preview yet.").font(.system(size: 13, weight: .medium))
            Text(model.devDetected.map { "Start \($0) and Keel points the preview at whatever URL it announces." }
                 ?? "This project has no dev command. Paste a URL above — a deploy URL works too.")
                .font(.system(size: 12)).foregroundStyle(.secondary)
        }
        .frame(maxWidth: 420, maxHeight: .infinity, alignment: .top)
        .padding(20)
    }

    private func go() {
        let url = Self.normalise(typed)
        guard !url.isEmpty else { return }
        typed = url
        editing = false
        model.previewURL = url
    }

    /// What someone types is not a URL yet. `localhost:3000` parses as a URL whose *scheme* is
    /// `localhost`, so it never throws and never loads; a bare number is a port.
    static func normalise(_ text: String) -> String {
        let t = text.trimmingCharacters(in: .whitespaces)
        if t.isEmpty { return t }
        if t.contains("://") { return t }
        if t.allSatisfy(\.isNumber) { return "http://127.0.0.1:\(t)" }
        return "http://" + t
    }
}


/// The width a page is rendered at, independent of how wide the pane happens to be.
enum PreviewWidth: String, CaseIterable, Identifiable {
    case desktop, tablet, phone
    var id: String { rawValue }

    /// Real CSS widths, not approximations: a site's breakpoints are written against these
    /// numbers, and rendering at 900 tells you about a layout nobody will ever see.
    var points: CGFloat {
        switch self {
        case .desktop: 1280
        case .tablet: 834
        case .phone: 390
        }
    }

    var icon: String {
        switch self {
        case .desktop: "display"
        case .tablet: "ipad"
        case .phone: "iphone"
        }
    }

    var title: String {
        switch self {
        case .desktop: "Desktop"
        case .tablet: "Tablet"
        case .phone: "Phone"
        }
    }
}
