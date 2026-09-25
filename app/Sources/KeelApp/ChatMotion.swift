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

/// AgentChrome's companion lens, rendered natively: the mark on a glass lens, ringed by living
/// light. Only this small view ticks while working.
///
/// It takes its mood from what the agent is doing — a slow drift while it thinks, a fast turn
/// while a command runs, amber and still while it waits on you — and a badge names the activity.
/// At rest it sits small and dim; when a turn ends it gives one pop, which is AgentChrome's
/// "found" beat. The halo is the one blur, on a 44pt square — negligible next to the per-frame
/// blur of a pane, which is what this used to cost when it was drawn twice at pane scale.
struct AgentAvatar: View {
    enum Mood: Equatable { case thinking, reading, running, editing, writing, agents, checking, waiting }

    var working: Bool
    var mood: Mood = .thinking
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var finished = 0

    private var held: Bool { mood == .waiting && working }
    private var light: AngularGradient {
        AngularGradient(colors: held ? K.C.held : K.C.light, center: .center)
    }
    /// Degrees a second. Waiting does not spin: the clock is stopped on the person.
    private var spin: Double {
        switch mood {
        case .waiting: 0
        case .thinking: 60
        case .reading: 150
        case .running, .checking: 220
        case .editing, .writing: 120
        case .agents: 100
        }
    }
    private var badge: String? {
        guard working else { return nil }
        return switch mood {
        case .thinking: "sparkles"
        case .reading: "magnifyingglass"
        case .running: "terminal.fill"
        case .editing: "pencil"
        case .writing: "text.cursor"
        case .agents: "person.2.fill"
        case .checking: "checkmark.seal.fill"
        case .waiting: "hand.raised.fill"
        }
    }

    var body: some View {
        let moving = working && !reduceMotion && !held
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !moving)) { timeline in
            let t = moving ? timeline.date.timeIntervalSinceReferenceDate : 0
            let turn = Angle.degrees((t * spin).truncatingRemainder(dividingBy: 360))
            ZStack {
                // The halo: the same light, blurred, breathing a little.
                Circle().fill(light)
                    .rotationEffect(turn)
                    .padding(-K.S.xs)
                    .blur(radius: 8)
                    .opacity(working ? (held ? 0.45 : 0.7 + sin(t * 1.8) * 0.12) : 0)
                // The ring: the lens sits on top and leaves 2pt of it showing.
                Circle().fill(light)
                    .rotationEffect(turn)
                    .opacity(working ? 1 : 0.5)
                lens
                Image(systemName: "sailboat.fill").font(K.F.row)
                    .foregroundStyle(working ? Color.white : K.C.accent.opacity(0.85))
                    .shadow(color: (held ? K.C.warn : K.C.accent).opacity(working ? 0.8 : 0), radius: 4)
                    // A boat rocks. Only while it moves; still at rest and for Reduce Motion.
                    .rotationEffect(.degrees(moving ? sin(t * 2.1) * 6 : 0), anchor: .bottom)
                    .offset(y: moving ? sin(t * 2.1 + 0.8) * 0.8 : 0)
            }
        }
        .frame(width: 34, height: 34)
        .scaleEffect(working ? 1 : 0.82)
        .animation(reduceMotion ? nil : K.M.enter, value: working)
        // One pop when a turn ends.
        .phaseAnimator([1.0, 1.1, 1.0], trigger: finished) { view, scale in
            view.scaleEffect(scale)
        } animation: { scale in scale > 1 ? .spring(response: 0.18, dampingFraction: 0.5) : K.M.enter }
        .onChange(of: working) { was, now in
            if was && !now && !reduceMotion { finished += 1 }
        }
        .overlay(alignment: .bottomTrailing) {
            if let badge {
                Image(systemName: badge)
                    .font(K.F.ui(7, .bold))
                    .foregroundStyle(K.C.onAccent)
                    .frame(width: 14, height: 14)
                    .background(held ? K.C.warn : K.C.accent, in: Circle())
                    .overlay(Circle().stroke(K.C.bg, lineWidth: 1.5))
                    .contentTransition(.symbolEffect(.replace))
                    .offset(x: 3, y: 3)
                    .transition(.scale.combined(with: .opacity))
            }
        }
        .animation(reduceMotion ? nil : K.M.quick, value: mood)
        .accessibilityLabel(working ? "Keel is working" : "Keel")
    }

    /// Dark glass: lit from the upper left, a gloss across the top, a hairline catch-light.
    private var lens: some View {
        Circle()
            .fill(RadialGradient(colors: [Color(white: 0.17), Color(white: 0.07), Color(white: 0.04)],
                                 center: UnitPoint(x: 0.35, y: 0.28), startRadius: 0, endRadius: 24))
            .overlay(alignment: .top) {
                Ellipse()
                    .fill(RadialGradient(colors: [Color.white.opacity(0.22), .clear],
                                         center: .center, startRadius: 0, endRadius: 10))
                    .frame(width: 20, height: 9)
                    .padding(.top, K.S.xxs)
            }
            .overlay(Circle().strokeBorder(
                LinearGradient(colors: [Color.white.opacity(0.2), .clear], startPoint: .top, endPoint: .center),
                lineWidth: 0.75))
            .padding(K.S.xxs)
    }
}

/// AgentChrome's edge light: while the agent is in control, light flows around the edge of the
/// window, and when it stops the light goes away. Three layers, as in its control shield — a
/// bright hairline, a soft glow, and a wide bloom that bleeds inward — each a square of gradient
/// turned by a repeating Core Animation and masked to the frame, so no SwiftUI body runs per
/// frame. The bloom turns the other way from the line, so the two drift against each other
/// rather than reading as one spinner, and it flares on every `beat` — a tool starting, a file
/// written — so the light answers what the agent does. Amber and slow while it waits on you.
struct AgentEdge: View {
    var active: Bool
    var held = false
    var beat = 0
    var radius: CGFloat
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack {
            if active {
                Glow(colors: held ? K.C.held : K.C.light, radius: radius,
                     period: held ? 10 : 5, beat: beat, still: reduceMotion)
                    .transition(.opacity)
            }
        }
        .animation(.easeInOut(duration: 0.9), value: active)
        .animation(.easeInOut(duration: 0.6), value: held)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private struct Glow: View {
        let colors: [Color]
        let radius: CGFloat
        let period: Double
        let beat: Int
        let still: Bool
        @State private var turned = false
        @State private var breathing = false

        var body: some View {
            GeometryReader { geo in
                ZStack {
                    light(1, geo.size).mask(RoundedRectangle(cornerRadius: radius).strokeBorder(lineWidth: 1.5))
                    light(1, geo.size).mask(RoundedRectangle(cornerRadius: radius).strokeBorder(lineWidth: 6))
                        .blur(radius: 10)
                        .opacity(0.85)
                    light(-1, geo.size).mask(RoundedRectangle(cornerRadius: radius).strokeBorder(lineWidth: 22))
                        .blur(radius: 36)
                        .opacity(breathing ? 0.3 : 0.55)
                        // The flare: the bloom widens and brightens for a moment, then settles.
                        .phaseAnimator([false, true], trigger: beat) { view, flare in
                            view.opacity(flare ? 1 : 0.55).scaleEffect(flare ? 1.015 : 1)
                        } animation: { flare in
                            flare ? .easeOut(duration: 0.18) : .easeInOut(duration: 1.1)
                        }
                }
                .drawingGroup()
            }
            .onAppear {
                guard !still else { return }
                withAnimation(.linear(duration: period).repeatForever(autoreverses: false)) { turned = true }
                withAnimation(.easeInOut(duration: 2.6).repeatForever(autoreverses: true)) { breathing = true }
            }
        }

        /// Wide enough to cover the frame at any angle. No division by the size: a
        /// `GeometryReader` proposes zero mid-animation.
        private func light(_ direction: Double, _ size: CGSize) -> some View {
            let side = size.width + size.height
            return Rectangle()
                .fill(AngularGradient(colors: colors, center: .center))
                .frame(width: side, height: side)
                .rotationEffect(.degrees(turned ? 360 * direction : 0))
                .frame(width: size.width, height: size.height)
        }
    }
}

/// What the agent just did, as a number that changes whenever it does something: a tool
/// started, a file written. `AgentEdge` flares on it. Its own view so that reading `calls` —
/// which changes with every argument fragment — invalidates this leaf and not the window.
struct AgentBeat: View {
    let model: SessionModel
    var radius: CGFloat

    var body: some View {
        let turn = model.turns.last
        AgentEdge(active: model.running && !model.following,
                  held: !model.pending.isEmpty,
                  beat: (turn?.calls.count ?? 0) + (turn?.files.count ?? 0),
                  radius: radius)
    }
}

/// A reply block arriving: out of a soft blur, up three points, the way AgentChrome's bubble words
/// do. Once per block, on appearance, and only for the turn being written — a replayed session
/// is already there and does not perform itself again.
struct Arrive: ViewModifier {
    var live: Bool
    @State private var shown = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        let settled = shown || !live
        content
            .opacity(settled ? 1 : 0)
            .blur(radius: settled ? 0 : 4)
            .offset(y: settled ? 0 : 3)
            .onAppear {
                guard live, !reduceMotion else { shown = true; return }
                withAnimation(.timingCurve(0.22, 1, 0.36, 1, duration: 0.52)) { shown = true }
            }
    }
}

/// AgentChrome's companion, in the transcript: the orb and a glass bubble naming what it is doing
/// now, sitting under the last thing it wrote, so it travels down the page with the reply.
///
/// Its own view with its own half-second clock, so the "quiet for 2m" label and the mood refresh
/// here without the turn above it re-rendering.
struct LiveTail: View {
    let model: SessionModel

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.5)) { _ in
            let now = WorkingBar(model: model).now
            let held = now.mood == .waiting
            HStack(spacing: K.S.sm) {
                AgentAvatar(working: true, mood: now.mood)
                Shimmer(active: !held) {
                    Text(now.label)
                        .font(K.F.small.weight(.medium))
                        .foregroundStyle(held ? K.C.warn : K.C.text)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                // Each new activity arrives in the bubble the way AgentChrome's words do.
                .id(now.label)
                .transition(.asymmetric(insertion: .opacity.combined(with: .offset(y: 3)),
                                        removal: .opacity))
                .padding(.horizontal, K.S.md)
                .frame(height: 28)
                .background(K.C.raised, in: Capsule())
                .overlay(Capsule().stroke(held ? K.C.warn.opacity(0.45) : K.C.line, lineWidth: 0.5))
                .shadow(color: K.C.shadow, radius: 8, y: 3)
                .animation(K.M.settle, value: now.label)
                Spacer(minLength: 0)
            }
        }
        .modifier(Arrive(live: true))
        .accessibilityElement(children: .combine)
    }
}

/// AgentChrome's beam, shrunk to a row: a band of light passing under a call while it runs.
/// A repeating Core Animation offset, so it costs a running row nothing per frame.
struct Beam: ViewModifier {
    var active: Bool
    @Environment(\.accessibilityReduceMotion) private var still
    @State private var across = false

    func body(content: Content) -> some View {
        content.background {
            if active && !still {
                GeometryReader { geo in
                    LinearGradient(colors: [.clear, K.C.accent.opacity(0.16), .clear],
                                   startPoint: .leading, endPoint: .trailing)
                        .frame(width: geo.size.width * 0.4)
                        .offset(x: across ? geo.size.width : -geo.size.width * 0.4)
                        .onAppear {
                            withAnimation(.easeInOut(duration: 1.5).repeatForever(autoreverses: false)) {
                                across = true
                            }
                        }
                }
                .clipShape(RoundedRectangle(cornerRadius: K.R.sm))
                .transition(.opacity)
            }
        }
    }
}

/// Words arriving. The glyphs written since the last flush come in lit — a soft glow in the
/// accent that settles into ink over half a second — instead of the paragraph growing by a
/// chunk. A `TextRenderer` draws the glyphs the layout already has, so nothing is re-laid-out
/// and the text stays selectable; `count` is animatable, and the glyphs past it are the new ones.
struct Reveal: TextRenderer, Animatable {
    /// Glyphs fully drawn. Everything past this is arriving.
    var count: Double
    var tone: Color
    var animatableData: Double {
        get { count }
        set { count = newValue }
    }

    func draw(layout: Text.Layout, in ctx: inout GraphicsContext) {
        var index = 0.0
        for line in layout {
            for run in line {
                for glyph in run {
                    // 0 at the edge of what has landed, 1 a few glyphs in: a wave, not a wall.
                    let age = min(max((count - index) / 3, 0), 1)
                    if age >= 1 {
                        ctx.draw(glyph)
                    } else {
                        var lit = ctx
                        lit.opacity = 0.15 + age * 0.85
                        lit.addFilter(.shadow(color: tone.opacity((1 - age) * 0.9), radius: 4 * (1 - age) + 1))
                        lit.draw(glyph)
                    }
                    index += 1
                }
            }
        }
    }
}

/// A block of prose that is being written to. Settled text draws plainly; while `live`, each
/// growth of the text animates `Reveal` from the old length to the new one.
struct StreamText: View {
    let text: AttributedString
    var live: Bool
    @State private var revealed: Double = 0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let length = Double(text.characters.count)
        if live && !reduceMotion {
            Text(text)
                .textRenderer(Reveal(count: revealed, tone: K.C.accent))
                .onChange(of: length, initial: true) { _, now in
                    withAnimation(.easeOut(duration: 0.55)) { revealed = now }
                }
        } else {
            Text(text)
        }
    }
}
