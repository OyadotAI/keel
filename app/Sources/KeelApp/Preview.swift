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
        "Change this element in the running app:\n\n" + describe() + "\n\n" + instruction
    }

    /// The element, described — without an instruction, so several can share one.
    func describe() -> String {
        var out = ""
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
        out += "\nmarkup:\n\(html)"
        return out
    }
}

/// A region of the page the agent's last write changed, as the page reported it.
struct Region: Decodable, Identifiable, Equatable {
    var selector: String
    var rect: Picked.Rect
    var tag: String
    var text: String
    var id: String { selector }
    static func == (a: Region, b: Region) -> Bool { a.selector == b.selector }
}

extension Picked.Rect: Equatable {
    /// The smallest rect holding all of these.
    static func union(_ rects: [Picked.Rect]) -> Picked.Rect? {
        guard let f = rects.first else { return nil }
        var x0 = f.x, y0 = f.y, x1 = f.x + f.width, y1 = f.y + f.height
        for r in rects.dropFirst() {
            x0 = min(x0, r.x); y0 = min(y0, r.y)
            x1 = max(x1, r.x + r.width); y1 = max(y1, r.y + r.height)
        }
        return Picked.Rect(x: x0, y: y0, width: x1 - x0, height: y1 - y0)
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
        view.navigationDelegate = context.coordinator
        view.load(URLRequest(url: url))
        context.coordinator.web = view
        context.coordinator.loaded = url

        // The page is the only thing that can photograph itself or draw on itself, so the
        // coordinator that owns it lends the model both. Cleared when the pane goes away, which is
        // what makes an un-photographable comparison report `unstable` instead of passing.
        let coordinator = context.coordinator
        let model = self.model
        // Off the update pass: writing observed state while SwiftUI is installing the view is
        // an invalidation loop, and one it does not always survive.
        Task { @MainActor in
            model.resnapshot = { [weak coordinator] rect in await coordinator?.snapshot(rect) }
            model.canvas = { [weak coordinator] message in coordinator?.send(message) }
        }
        return view
    }

    static func dismantleNSView(_ view: WKWebView, coordinator: Coordinator) {
        let model = coordinator.model
        Task { @MainActor in
            model.resnapshot = nil
            model.canvas = nil
        }
        view.navigationDelegate = nil
        view.configuration.userContentController.removeScriptMessageHandler(forName: "keel")
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

        // The reload button used to re-poll the daemon and never touch the page. Now it asks
        // the page to load again, which is what everyone pressing it meant.
        if context.coordinator.reloaded != model.reloadTick {
            context.coordinator.reloaded = model.reloadTick
            let model = self.model
            Task { @MainActor in model.previewProblem = nil }
            view.reload()
        }

        // Only on a change: this runs on every render pass, and re-posting the mode each time
        // was a message per keystroke to every frame.
        if context.coordinator.picking != picking {
            context.coordinator.picking = picking
            context.coordinator.send(["keel": picking ? "pick-on" : "pick-off"])
        }
    }

    @MainActor
    final class Coordinator: NSObject, WKScriptMessageHandler, WKNavigationDelegate {
        let model: SessionModel
        weak var web: WKWebView?
        /// What the view was last told to load, so an unchanged address is not reloaded on every
        /// pass of `updateNSView` — which would restart the page under you on every keystroke.
        var loaded: URL?
        var picking = false
        var reloaded = 0
        init(model: SessionModel) { self.model = model }

        /// A page that did not load says so, over the pane, rather than staying white.
        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            model.previewProblem = error.localizedDescription
            Task { await model.refreshDev() }
        }
        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            model.previewProblem = error.localizedDescription
        }

        /// A message to the canvas script, in every frame — the one the dev server owns is the
        /// one that matters, and it is not the main frame.
        func send(_ message: [String: Any]) {
            guard let web,
                  let data = try? JSONSerialization.data(withJSONObject: message),
                  let json = String(data: data, encoding: .utf8) else { return }
            web.evaluateJavaScript("""
                (function(){
                  var m = \(json);
                  window.postMessage(m, '*');
                  for (var i=0;i<window.frames.length;i++) {
                    try { window.frames[i].postMessage(m, '*'); } catch(e){}
                  }
                })();
                """)
        }

        func userContentController(_ c: WKUserContentController, didReceive message: WKScriptMessage) {
            guard let dict = message.body as? [String: Any],
                  let data = try? JSONSerialization.data(withJSONObject: dict) else { return }
            switch dict["type"] as? String {
            case "changed":
                struct Changed: Decodable { var regions: [Region] }
                if let c = try? JSONDecoder().decode(Changed.self, from: data) {
                    model.regionsChanged(c.regions)
                }
            default:
                if let picked = try? JSONDecoder().decode(Picked.self, from: data) {
                    Task { await capture(picked) }
                }
            }
        }

        /// The page loaded — or reloaded under a turn. Re-draw the pins, and if the agent is
        /// mid-edit, watch for the change to land: the `expect` sent before this load was lost
        /// with the old document.
        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            model.previewProblem = nil
            model.syncCanvas()
            if model.running, model.editing != nil { send(["keel": "expect"]) }
            // A page that loaded and painted nothing is the other blank: a 200 with an empty
            // body, a client render that threw, a framework error overlay that is itself blank.
            // Give it two seconds, then ask.
            Task { [weak self, weak webView] in
                try? await Task.sleep(for: .seconds(2))
                guard let self, let webView else { return }
                let js = "(document.body && document.body.innerText.trim().length) || 0"
                if let n = try? await webView.evaluateJavaScript(js) as? Int, n == 0 {
                    await self.model.refreshDev()
                    self.model.previewProblem = "The page loaded but rendered nothing."
                }
            }
        }

        /// A rect of the page, as it is right now — or nothing, never a trap.
        ///
        /// Two things about `takeSnapshot` that crashed the app on other people's machines:
        /// its result is declared `_Nullable` rather than `_Nullable_result`, so the async
        /// import is a non-optional `NSImage` and the generated thunk force-unwraps the nil
        /// WebKit hands back; and WebKit hands back nil for any rect outside the view's bounds,
        /// which a viewport-relative rect from a scrolled page routinely is. So: the completion
        /// form, through our own continuation, on a rect clamped to the bounds.
        func snapshot(_ rect: Picked.Rect) async -> NSImage? {
            guard let web else { return nil }
            let wanted = CGRect(x: rect.x, y: rect.y, width: rect.width, height: rect.height)
            let clamped = wanted.intersection(web.bounds)
            guard !clamped.isNull, clamped.width > 1, clamped.height > 1 else { return nil }
            let config = WKSnapshotConfiguration()
            config.rect = clamped
            return await withCheckedContinuation { (k: CheckedContinuation<NSImage?, Never>) in
                web.takeSnapshot(with: config) { image, _ in k.resume(returning: image) }
            }
        }

        /// The before image, cropped to the element.
        ///
        /// `takeSnapshot` with a rect is exact and native — no screen recording permission, no
        /// window capture, and it sees the element as the page actually rendered it. This is the
        /// half nobody else has: a UI change gets a pixel diff, not only a text one.
        private func capture(_ picked: Picked) async {
            model.designPick(picked, before: await snapshot(picked.rect))
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
            if let file = model.editing {
                // The agent is writing the page you are looking at. Said here, over the page,
                // so the ripple that follows is not a surprise.
                HStack(spacing: K.S.sm) {
                    Sweep()
                    Text("Editing \((file as NSString).lastPathComponent)…")
                        .font(K.F.mono(11)).foregroundStyle(K.C.text).lineLimit(1)
                    if let route = Frontend.route(for: file) {
                        Text(route).font(K.F.mono(10)).foregroundStyle(K.C.faint)
                    }
                    Spacer()
                }
                .padding(.horizontal, K.S.md).padding(.vertical, 5)
                .background(K.C.accent.opacity(0.10))
                Hairline()
            } else if !model.changedRegions.isEmpty {
                HStack(spacing: K.S.sm) {
                    Image(systemName: "sparkles").font(.system(size: 10)).foregroundStyle(K.C.accent)
                    Text("\(model.changedRegions.count) region\(model.changedRegions.count == 1 ? "" : "s") changed — "
                         + "click a dot to pin a note there")
                        .font(K.F.small).foregroundStyle(K.C.dim)
                    Spacer()
                    Button("Clear") { model.clearRegions() }.buttonStyle(QuietButton())
                }
                .padding(.horizontal, K.S.md).padding(.vertical, 4)
                .background(K.C.accent.opacity(0.06))
                Hairline()
            }
            if let problem = model.previewProblem {
                PreviewProblem(model: model, problem: problem)
                Hairline()
            }
            if let s = model.previewURL, let url = URL(string: s) {
                // The page is rendered at a real width and scaled to fit, rather than squeezed
                // into the pane. The pane is 340–720pt, so a responsive site was correctly
                // rendering its phone layout — and there was no way to ask for anything else.
                GeometryReader { geo in
                    // A zero-width proposal arrives on the first layout and mid-animation. It
                    // made `scale` zero, the height infinite, and the layer geometry invalid —
                    // the crash on the way into the Designer. Nothing is drawn until there is
                    // room to draw it in, and the scale never reaches zero.
                    if geo.size.width > 8, geo.size.height > 8 {
                        let target = model.previewWidth.points
                        let scale = max(0.05, min(1, (geo.size.width - 2) / target))
                        PreviewPane(url: url, model: model, picking: $model.picking)
                            .frame(width: target, height: geo.size.height / scale)
                            .scaleEffect(scale, anchor: .top)
                            .frame(width: geo.size.width, height: geo.size.height, alignment: .top)
                            .clipped()
                    } else {
                        Color.clear
                    }
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
            .help("Click an element in the page to pin a note on it for the agent")

            Toggle(isOn: $model.followEdits) {
                Label("Follow", systemImage: "eye")
            }
            .toggleStyle(.button)
            .controlSize(.small)
            .help("Bring this pane forward and go to the page whenever the agent edits the "
                  + "frontend")

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
                model.reloadTick += 1
                Task { await model.refreshDev() }
            } label: {
                Image(systemName: "arrow.clockwise").font(.system(size: 10))
                    .frame(width: 20, height: 18).contentShape(Rectangle())
            }
            .buttonStyle(.plain).foregroundStyle(K.C.faint)
            .hint("Reload the page (⌘R)")
            .keyboardShortcut("r", modifiers: .command)
        }
        .padding(8)
    }

    private var empty: some View {
        VStack(alignment: .leading, spacing: K.S.half) {
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


/// Why the pane is blank, with the dev server's own last words underneath.
///
/// "It said it finished but the preview is white" was the report. White is what a page that
/// failed to load, a page that threw during render, and a dev server that died all look like.
/// The difference is in the error and in the log, so both go on screen.
private struct PreviewProblem: View {
    let model: SessionModel
    let problem: String
    @State private var showLog = false

    /// The lines worth reading: the last ones, and any that say error.
    private var lines: [String] {
        let all = model.devLog
        let bad = all.filter { $0.range(of: "error", options: .caseInsensitive) != nil }
        return Array((bad.suffix(6) + all.suffix(6)).reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } })
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 10)).foregroundStyle(K.C.warn)
                Text(problem).font(K.F.small).foregroundStyle(K.C.text)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer()
                if !model.devRunning, model.devDetected != nil {
                    Button("Start the dev server") { Task { await model.startDev() } }
                        .buttonStyle(QuietButton(tone: K.C.accent))
                } else {
                    Button("Reload") { model.reloadTick += 1 }.buttonStyle(QuietButton())
                }
                if !lines.isEmpty {
                    Button(showLog ? "Hide output" : "Server output") { showLog.toggle() }
                        .buttonStyle(QuietButton())
                }
            }
            if !model.devRunning {
                Text(model.devDetected == nil
                     ? "No dev server is running and this project declares no dev command."
                     : "The dev server is not running.")
                    .font(K.F.micro).foregroundStyle(K.C.dim)
            }
            if showLog {
                ScrollView {
                    Text(lines.joined(separator: "\n"))
                        .font(K.F.codeSmall).foregroundStyle(K.C.dim)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 160)
                .padding(K.S.sm)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .background(K.C.warn.opacity(0.08))
    }
}
