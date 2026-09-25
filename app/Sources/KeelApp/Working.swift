import SwiftUI

/// Proof that the agent is alive.
///
/// A small grey "working…" is indistinguishable from a hung process, and a turn can legitimately
/// spend two minutes inside one command. The three things that answer "is it stuck" are a clock
/// that visibly ticks, the name of what it is doing right now, and motion that is driven by real
/// events rather than by a spinner that would keep spinning after a crash.
struct WorkingBar: View {
    let model: SessionModel

    var body: some View {
        // A timeline rather than a repeating animation: the clock has to advance on its own, and
        // seeing the seconds move is the whole reason this is here.
        TimelineView(.periodic(from: .now, by: 0.5)) { context in
            HStack(spacing: K.S.sm) {
                // The orb and what it is doing live in the transcript now (`LiveTail`), under the
                // words being written. The bar keeps what is worth a glance from anywhere: the
                // counts, the clock and Stop.
                Sweep()
                marks
                Spacer(minLength: K.S.sm)

                Text(elapsed(at: context.date))
                    .font(K.F.codeSmall.weight(.medium))
                    .monospacedDigit()
                    .foregroundStyle(K.C.accent)

                // Not for a turn running in somebody's terminal: the daemon has no process to
                // signal for it, so the button would answer "Could not stop the turn".
                if !model.following {
                    Button("Stop") { model.stop() }
                        .buttonStyle(QuietButton(tone: K.C.del))
                        .keyboardShortcut(.escape, modifiers: [])
                        .help("Interrupts the agent and whatever it started, the way ⌃C does — the "
                              + "turn ends rather than being abandoned (⌘. or Esc)")
                }
            }
            .padding(.horizontal, K.S.md)
            .padding(.vertical, K.S.half)
            .background(waiting ? K.C.warn.wash : K.C.accent.wash,
                        in: RoundedRectangle(cornerRadius: K.R.lg))
            .overlay(
                RoundedRectangle(cornerRadius: K.R.lg)
                    .stroke(waiting ? K.C.warn.opacity(0.35) : K.C.accent.opacity(0.25),
                            lineWidth: 1)
            )
        }
    }

    // MARK: What matters so far

    /// The things worth a glance while it runs, counted from the turn itself: what it changed,
    /// what it ran, what failed, and who is waiting on whom. Nothing is shown at zero, so a quiet
    /// turn has a quiet bar.
    @ViewBuilder
    private var marks: some View {
        if let turn = model.current {
            let files = turn.files.count
            let commands = turn.calls.count { $0.tool == "Bash" }
            let failed = turn.calls.count(where: \.failed)
            let agents = turn.calls.count { Self.agentTools.contains($0.tool) }
            if files + commands + failed + agents > 0 || waiting {
                HStack(spacing: K.S.xs) {
                    if waiting { Mark(symbol: "hand.raised.fill", text: "needs you", tone: K.C.warn) }
                    if failed > 0 { Mark(symbol: "xmark.circle.fill", text: "\(failed) failed", tone: K.C.del) }
                    if files > 0 { Mark(symbol: "doc.fill", text: Self.plural(files, "file"), tone: K.C.dim) }
                    if commands > 0 { Mark(symbol: "terminal.fill", text: Self.plural(commands, "command"), tone: K.C.dim) }
                    if agents > 0 { Mark(symbol: "person.2.fill", text: Self.plural(agents, "agent"), tone: K.C.dim) }
                }
                .transition(.opacity)
            }
        }
    }

    private static func plural(_ n: Int, _ word: String) -> String { "\(n) \(word)\(n == 1 ? "" : "s")" }

    // MARK: What it is doing now

    static let agentTools: Set<String> = ["Task", "Agent"]
    private static let readTools: Set<String> = ["Read", "Glob", "Grep", "LS", "WebFetch", "WebSearch", "NotebookRead"]
    private static let editTools: Set<String> = ["Edit", "MultiEdit", "Write", "NotebookEdit", "Update"]

    /// What it is doing, named, and the mood the avatar takes from it. "Working" says nothing;
    /// "Running cargo test" says everything. And the states that used to read as "thinking…" for
    /// ten minutes: waiting on the person, a command that has gone quiet, and agents working in
    /// the background.
    /// Also read by `LiveTail`, which draws it in the transcript.
    var now: (label: String, mood: AgentAvatar.Mood) {
        // Before the agent exists there are no events to be quiet, so the "no output" clock below
        // must not start. Checking out a repository is work, and it says which work.
        if let preparing = model.preparing { return (preparing, .thinking) }
        guard let turn = model.current else { return ("starting…", .thinking) }
        // The card above says what is being asked and offers the answers; this only has to say
        // that the clock is stopped on it.
        if !model.pending.isEmpty {
            return (model.pending.count == 1
                ? "paused — waiting for your answer"
                : "paused — \(model.pending.count) questions waiting", .waiting)
        }
        // A question asked of somebody else: the terminal, or the window that owns the turn.
        // Nothing here can answer it, and the clock is stopped on it all the same.
        if model.following, let ask = turn.asked.last, ask.decision == nil {
            return ("paused — waiting on a question in the window that owns it", .waiting)
        }
        // The agent's own silence, not the connection's: `lastEventAt` moves on every heartbeat
        // now, so it would read "quiet for 0s" through an entire five-minute test run.
        let quiet = Int(Date().timeIntervalSince(model.lastProgressAt))
        // The agent has stopped and the daemon is running the project's checks. The stream
        // carries one fact when the gate starts and nothing until it ends, so the fact that opened
        // the gate is the last progress, and `quiet` is how long it has been running.
        if case .running(let command) = turn.gate {
            return ("Checking — \(command), \(Self.clock(quiet))", .checking)
        }
        // A finished call is not what is happening now: once the last one has returned the agent
        // is thinking or writing, and naming the call said it was still running.
        if let call = turn.calls.last(where: \.running) {
            let (verb, mood) = Self.verb(for: call)
            let name = call.subject.isEmpty ? verb : "\(verb) \(call.subject)"
            // It names what is quiet and for how long; Stop is already the next control along.
            return (quiet > 90 ? "\(name) — quiet for \(Self.clock(quiet))" : name, mood)
        }
        // Agents started in the background returned at once, and the turn stays open until they
        // report. This read "writing…" for as long as they ran.
        let background = turn.calls.count {
            guard Self.agentTools.contains($0.tool), case .bool(true)? = $0.input["run_in_background"] else { return false }
            return true
        }
        if background > 0, quiet > 5 {
            return (background == 1 ? "An agent is working in the background"
                                    : "\(background) agents working in the background", .agents)
        }
        if quiet > 90 { return ("quiet for \(Self.clock(quiet))", .thinking) }
        return turn.text.isEmpty ? ("Thinking…", .thinking) : ("Writing…", .writing)
    }

    private static func verb(for call: Turn.Call) -> (String, AgentAvatar.Mood) {
        switch call.tool {
        case "Bash": ("Running", .running)
        case "Grep", "Glob", "WebSearch": ("Searching", .reading)
        case let t where readTools.contains(t): ("Reading", .reading)
        case "Write": ("Writing", .editing)
        case let t where editTools.contains(t): ("Editing", .editing)
        case let t where agentTools.contains(t): ("Agent —", .agents)
        default: (call.tool, .running)
        }
    }

    private static func clock(_ s: Int) -> String { s >= 60 ? "\(s / 60)m \(s % 60)s" : "\(s)s" }

    private var waiting: Bool { !model.pending.isEmpty }

    private func elapsed(at now: Date) -> String {
        guard let started = model.current?.started else { return "0s" }
        let s = Int(now.timeIntervalSince(started))
        guard s >= 60 else { return "\(s)s" }
        return "\(s / 60)m \((s % 60).formatted(.number.precision(.integerLength(2))))s"
    }
}

/// One counted thing on the working bar.
private struct Mark: View {
    let symbol: String
    let text: String
    let tone: Color

    var body: some View {
        HStack(spacing: K.S.xxs) {
            Image(systemName: symbol).font(K.F.ui(8, .semibold))
            Text(text).font(K.F.micro).monospacedDigit()
        }
        .foregroundStyle(tone)
        .padding(.horizontal, K.S.xs).padding(.vertical, K.S.hair)
        .background(tone.opacity(0.1), in: Capsule())
        .contentTransition(.numericText())
    }
}

/// A band of light that runs across what the agent is doing — the "it is reading this right
/// now" cue every agent UI has converged on. A repeating animation on one small label, not a
/// timeline: Core Animation drives it, so it costs the transcript nothing. Reduce Motion gets
/// the label still.
struct Shimmer<Content: View>: View {
    var active: Bool
    @ViewBuilder var content: Content
    @Environment(\.accessibilityReduceMotion) private var still
    @State private var phase: CGFloat = -1

    var body: some View {
        if active && !still {
            content
                .overlay {
                    GeometryReader { geo in
                        LinearGradient(colors: [.clear, Color.white.opacity(0.55), .clear],
                                       startPoint: .leading, endPoint: .trailing)
                            .frame(width: max(geo.size.width * 0.35, 1))
                            .offset(x: phase * max(geo.size.width, 1))
                    }
                    .blendMode(.plusLighter)
                    .mask(content)
                    .allowsHitTesting(false)
                }
                .onAppear {
                    withAnimation(.linear(duration: 1.6).repeatForever(autoreverses: false)) { phase = 1.2 }
                }
        } else {
            content
        }
    }
}

/// A bar that sweeps. Indeterminate on purpose — a turn has no total to be a fraction of, and a
/// progress bar that invents one is a lie you watch for two minutes.
struct Sweep: View {
    @Environment(\.accessibilityReduceMotion) private var still

    var body: some View {
        // A person who asked for less motion gets the bar without the sweep. The clock beside it
        // still ticks, which is the part that proves the agent is alive.
        if still {
            Capsule().fill(K.C.accent.opacity(0.5)).frame(width: 44, height: 3)
        } else { moving }
    }

    private var moving: some View {
        TimelineView(.animation) { context in
            let t = context.date.timeIntervalSinceReferenceDate
            let phase = (t.truncatingRemainder(dividingBy: 1.4)) / 1.4

            Capsule()
                .fill(K.C.accent.opacity(0.25))
                .frame(width: 44, height: 3)
                .overlay(alignment: .leading) {
                    Capsule()
                        .fill(K.C.accent)
                        .frame(width: 14, height: 3)
                        // Eased at both ends so it reads as motion rather than as a marquee.
                        .offset(x: 30 * (1 - cos(phase * 2 * .pi)) / 2)
                }
        }
    }
}
