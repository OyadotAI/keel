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

                Button("Stop") { model.stop() }
                    .buttonStyle(QuietButton(tone: K.C.del))
                    .keyboardShortcut(.escape, modifiers: [])
                    .help("Interrupts the agent and whatever it started, the way ⌃C does — the "
                          + "turn ends rather than being abandoned (⌘. or Esc)")
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
        guard let turn = model.current else { return "starting…" }
        // The card above says what is being asked and offers the answers; this only has to say
        // that the clock is stopped on it.
        if !model.pending.isEmpty {
            return model.pending.count == 1
                ? "paused — waiting for your answer"
                : "paused — \(model.pending.count) questions waiting"
        }
        let quiet = Int(Date().timeIntervalSince(model.lastEventAt))
        if let call = turn.calls.last(where: \.running) ?? turn.calls.last {
            let name = call.subject.isEmpty ? call.tool : "\(call.tool)  \(call.subject)"
            return quiet > 90 ? "\(name) — no output for \(quiet / 60)m \(quiet % 60)s; a server that never exits? Stop and run it from the terminal" : name
        }
        if quiet > 90 { return "no output for \(quiet / 60)m \(quiet % 60)s — stop and try again, or ask in the terminal" }
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
