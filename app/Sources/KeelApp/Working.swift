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
                    .font(K.F.mono(11))
                    .foregroundStyle(K.C.text)
                    .lineLimit(1)
                    .truncationMode(.head)

                Spacer(minLength: K.S.sm)

                Text(elapsed(at: context.date))
                    .font(K.F.mono(11, .medium))
                    .monospacedDigit()
                    .foregroundStyle(K.C.accent)

                Button("Stop") { model.stop() }
                    .buttonStyle(QuietButton(tone: K.C.del))
                    .help("Sends SIGINT — the turn ends rather than being abandoned (⌘.)")
            }
            .padding(.horizontal, K.S.md)
            .padding(.vertical, 6)
            .background(K.C.accent.opacity(0.10))
            .overlay(alignment: .bottom) { Hairline() }
        }
    }

    /// What it is doing, named. "Working" says nothing; `Bash cargo test` says everything.
    private var activity: String {
        guard let turn = model.current else { return "starting…" }
        if let call = turn.calls.last(where: \.running) ?? turn.calls.last {
            return call.subject.isEmpty ? call.tool : "\(call.tool)  \(call.subject)"
        }
        return turn.text.isEmpty ? "thinking…" : "writing…"
    }

    private func elapsed(at now: Date) -> String {
        guard let started = model.current?.started else { return "0s" }
        let s = Int(now.timeIntervalSince(started))
        return s < 60 ? "\(s)s" : String(format: "%dm %02ds", s / 60, s % 60)
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
