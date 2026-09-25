import AppKit
import SwiftUI

/// One continuous follower, retargeted as text grows. It moves the native clip view without
/// rebuilding SwiftUI rows or starting a fresh animation for every streamed batch.
@MainActor
final class ChatTailMotion {
    weak var scroll: NSScrollView?
    private var timer: Timer?
    private var lastFrame = Date.timeIntervalSinceReferenceDate

    func follow(reduceMotion: Bool) {
        guard let scroll, scroll.window != nil else { return }
        if reduceMotion {
            stop()
            move(to: end(of: scroll), in: scroll)
            return
        }
        guard timer == nil else { return }
        lastFrame = Date.timeIntervalSinceReferenceDate
        let timer = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.frame() }
        }
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    func stop() { timer?.invalidate(); timer = nil }

    private func end(of scroll: NSScrollView) -> CGFloat {
        max(0, (scroll.documentView?.frame.height ?? 0) - scroll.contentView.bounds.height)
    }

    private func move(to y: CGFloat, in scroll: NSScrollView) {
        scroll.contentView.scroll(to: NSPoint(x: scroll.contentView.bounds.minX, y: y))
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    private func frame() {
        guard let scroll, scroll.window != nil else { stop(); return }
        let now = Date.timeIntervalSinceReferenceDate
        let current = scroll.contentView.bounds.minY
        let target = end(of: scroll)
        let next = Self.advance(current, toward: target, elapsed: now - lastFrame)
        lastFrame = now
        move(to: next, in: scroll)
        if abs(target - next) < 0.5 { move(to: target, in: scroll); stop() }
    }

    /// Exponential settling preserves continuity when another line arrives mid-flight.
    static func advance(_ current: CGFloat, toward target: CGFloat, elapsed: Double) -> CGFloat {
        current + (target - current) * (1 - exp(-min(max(elapsed, 0), 0.05) / 0.065))
    }
}

struct ChatScrollProbe: NSViewRepresentable {
    let motion: ChatTailMotion
    func makeNSView(context: Context) -> Probe { Probe(motion: motion) }
    func updateNSView(_ view: Probe, context: Context) { view.connect() }
    static func dismantleNSView(_ view: Probe, coordinator: ()) { view.motion.stop() }
    final class Probe: NSView {
        let motion: ChatTailMotion
        init(motion: ChatTailMotion) { self.motion = motion; super.init(frame: .zero) }
        required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
        override func viewDidMoveToWindow() { super.viewDidMoveToWindow(); connect() }
        func connect() { motion.scroll = enclosingScrollView }
        override func hitTest(_ point: NSPoint) -> NSView? { nil }
    }
}

/// AgentChrome's companion lens, rendered natively. Only this small view ticks while working.
struct AgentAvatar: View {
    var working: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !working || reduceMotion)) { timeline in
            let phase = working && !reduceMotion ? timeline.date.timeIntervalSinceReferenceDate : 0
            ZStack {
                Circle().fill(K.C.accent.opacity(working ? 0.22 : 0.06))
                    .blur(radius: 5).scaleEffect(working ? 1.15 + sin(phase * 2) * 0.07 : 1)
                Circle().fill(AngularGradient(colors: [K.C.accent, K.C.accent.opacity(0.12),
                    Color.white.opacity(0.85), K.C.accent], center: .center))
                    .rotationEffect(.degrees(phase * 100))
                Circle().fill(RadialGradient(colors: [Color(white: 0.20), Color(white: 0.055)],
                    center: .topLeading, startRadius: 0, endRadius: 30)).padding(K.S.xxs)
                Ellipse().fill(Color.white.opacity(0.15)).frame(width: 17, height: 7)
                    .blur(radius: 2).offset(y: -8)
                Image(systemName: "sailboat.fill").font(K.F.row)
                    .foregroundStyle(K.C.accent.opacity(working ? 1 : 0.75))
            }
        }
        .frame(width: 34, height: 34)
        .accessibilityLabel(working ? "Keel is working" : "Keel")
    }
}
