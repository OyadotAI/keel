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
                Sweep()

                Text(activity)
                    .font(K.F.codeSmall)
                    .foregroundStyle(K.C.text)
                    .lineLimit(1)
                    .truncationMode(.middle)

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

    /// What it is doing, named. "Working" says nothing; `Bash cargo test` says everything.
    /// And the two states that used to read as "thinking…" for ten minutes: waiting on the
    /// person, and a command that has gone quiet.
    private var activity: String {
        // Before the agent exists there are no events to be quiet, so the "no output" clock below
        // must not start. Checking out a repository is work, and it says which work.
        if let preparing = model.preparing { return preparing }
        guard let turn = model.current else { return "starting…" }
        // The card above says what is being asked and offers the answers; this only has to say
        // that the clock is stopped on it.
        if !model.pending.isEmpty {
            return model.pending.count == 1
                ? "paused — waiting for your answer"
                : "paused — \(model.pending.count) questions waiting"
        }
        // A question asked of somebody else: the terminal, or the window that owns the turn.
        // Nothing here can answer it, and the clock is stopped on it all the same.
        if model.following, let ask = turn.asked.last, ask.decision == nil {
            return "paused — waiting on a question in the window that owns it"
        }
        // The agent's own silence, not the connection's: `lastEventAt` moves on every heartbeat
        // now, so it would read "quiet for 0s" through an entire five-minute test run.
        let quiet = Int(Date().timeIntervalSince(model.lastProgressAt))
        if let call = turn.calls.last(where: \.running) ?? turn.calls.last {
            let name = call.subject.isEmpty ? call.tool : "\(call.tool)  \(call.subject)"
            // It used to end "Stop and run it from the terminal", which is Keel telling you to
            // go and use the thing Keel exists to replace — and saying it at ninety seconds, when
            // a build or a test run has every right to be quiet. It names what is quiet and for
            // how long; Stop is already the next control along.
            return quiet > 90 ? "\(name) — quiet for \(quiet / 60)m \(quiet % 60)s" : name
        }
        if quiet > 90 { return "quiet for \(quiet / 60)m \(quiet % 60)s" }
        return turn.text.isEmpty ? "thinking…" : "writing…"
    }
    private var waiting: Bool { !model.pending.isEmpty }

    private func elapsed(at now: Date) -> String {
        guard let started = model.current?.started else { return "0s" }
        let s = Int(now.timeIntervalSince(started))
        guard s >= 60 else { return "\(s)s" }
        return "\(s / 60)m \((s % 60).formatted(.number.precision(.integerLength(2))))s"
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
