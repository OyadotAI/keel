import SwiftUI

/// A command the agent is blocked on, asked where you are already looking.
///
/// Never a modal. The agent is genuinely stopped here — the `PreToolUse` hook holds the call open
/// for up to four minutes — but a sheet over the window would stop you too, and the whole design
/// is that nothing blocks the person.
///
/// Trust is first and largest because it is what someone working on their own repository actually
/// wants. Approving one program at a time is the toll that drives people to
/// `--dangerously-skip-permissions`, which turns permissions off everywhere and permanently.
struct ApprovalCard: View {
    let pending: Wire.Pending
    let model: SessionModel

    /// One rule named, the rest counted.
    ///
    /// Every rule was joined into the label, so a shell loop — which derives one rule per program
    /// in it — wrapped the button onto a second line, and that button then set the height of the
    /// whole row while the others sat centred against it. The full list is in the tooltip.
    private var allowLabel: String {
        guard let first = pending.rules.first else { return "Allow" }
        let more = pending.rules.count - 1
        return more > 0 ? "Allow \(first) +\(more)" : "Allow \(first)"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            // What to do, not what is happening. "WAITING FOR YOU" in tracked amber capitals was
            // the state — and the bar below now says the state, in amber, so this said it twice.
            HStack(spacing: K.S.sm) {
                Image(systemName: "hand.raised.fill")
                    .font(K.F.micro).foregroundStyle(K.C.warn)
                    .accessibilityHidden(true)
                Text(pending.command.isEmpty ? "Let the agent use \(pending.tool)?"
                                             : "Let the agent run this?")
                    .font(K.F.body.weight(.semibold)).foregroundStyle(K.C.text)
                Spacer(minLength: 0)
            }

            Text(pending.command.isEmpty ? pending.tool : pending.command)
                .font(K.F.code)
                .foregroundStyle(K.C.text)
                .textSelection(.enabled)
                // Belt and braces with the daemon's own cap. `fixedSize` asks the layout to
                // measure the whole string at once, and a command carrying a heredoc hung the
                // main thread inside CoreText for seconds — on the one card that must appear
                // instantly, because the turn has stopped and is waiting on it.
                .lineLimit(24)
                .fixedSize(horizontal: false, vertical: true)
                .padding(K.S.sm)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))

            // One filled button, the same one every primary action in the app uses: trust is
            // the answer that removes the whole class of questions, so it is the one that
            // stands out. The narrower answers are quiet beside it; deny sits apart, in red.
            HStack(spacing: K.S.sm) {
                Button("Trust this project") { model.answer(pending, allow: true, scope: "trust") }
                    .buttonStyle(FilledButton())
                    .help("Stop asking in this project. Withdrawable any time from the status bar.")
                Button(allowLabel) { model.answer(pending, allow: true, scope: "project") }
                    .buttonStyle(QuietButton())
                    .help("Remembered in .keel/permissions.json for this project: "
                          + pending.rules.joined(separator: ", "))
                Button("Once") { model.answer(pending, allow: true, scope: "session") }
                    .buttonStyle(QuietButton())
                    .help("This conversation only, until Keel restarts")
                Spacer()
                Button("Deny") { model.answer(pending, allow: false, scope: "session") }
                    .buttonStyle(QuietButton(tone: K.C.del))
                    .help("The agent is told to stop rather than substitute another command")
            }
        }
        // A card, not a warning block. The full amber wash was competing with the amber working
        // bar directly below it for the same fact; the amber that is left is the icon and the
        // edge, which is enough to say this one is different from the composer under it.
        .padding(K.S.md)
        .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.lg))
        .overlay(
            RoundedRectangle(cornerRadius: K.R.lg).stroke(K.C.warn.opacity(0.45), lineWidth: 1)
        )
        // The arrival is the parent's `.transition` with `K.M.enter`, not a `@State` flag flipped
        // in `onAppear`. That version started at `opacity(0)` and depended on `onAppear` firing to
        // become visible at all — which it does not in an offscreen render, and which made the one
        // card in the app that must never be missed the one card that could fail to draw.
        .shadow(color: .black.opacity(0.18), radius: 12, y: 3)
    }
}


// MARK: - Questions

/// A question the agent asked, answered where you are already looking.
///
/// Headless `claude -p` has no terminal to ask on, so `AskUserQuestion` waited sixty seconds and
/// carried on without an answer — the most-upvoted complaint against the CLI, and one Keel can
/// answer with the same hook that holds a command. The turn is paused here until you pick.
struct QuestionCard: View {
    let pending: Wire.Pending
    let model: SessionModel
    @State private var chosen: [String: Set<String>] = [:]
    @State private var other: [String: String] = [:]

    private var questions: [Wire.Question] { pending.questions }

    /// Every question answered, by a choice or by typing.
    private var complete: Bool {
        questions.allSatisfy { q in
            !(chosen[q.id] ?? []).isEmpty
                || !(other[q.id] ?? "").trimmingCharacters(in: .whitespaces).isEmpty
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "questionmark.bubble.fill")
                    .font(K.F.micro).foregroundStyle(K.C.warn)
                    .accessibilityHidden(true)
                Text(questions.count == 1 ? "The agent is asking" : "The agent is asking you \(questions.count) things")
                    .font(K.F.body.weight(.semibold)).foregroundStyle(K.C.text)
                Spacer(minLength: 0)
            }

            ForEach(questions) { q in
                VStack(alignment: .leading, spacing: K.S.xs) {
                    Text(q.text).font(K.F.body).foregroundStyle(K.C.text)
                        .fixedSize(horizontal: false, vertical: true)
                    ForEach(Array(q.options.enumerated()), id: \.offset) { i, o in
                        let on = chosen[q.id, default: []].contains(o.label)
                        Button {
                            if q.multiSelect {
                                if on { chosen[q.id, default: []].remove(o.label) }
                                else { chosen[q.id, default: []].insert(o.label) }
                            } else {
                                chosen[q.id] = [o.label]
                            }
                        } label: {
                            HStack(alignment: .firstTextBaseline, spacing: K.S.sm) {
                                Image(systemName: on
                                      ? (q.multiSelect ? "checkmark.square.fill" : "largecircle.fill.circle")
                                      : (q.multiSelect ? "square" : "circle"))
                                    .font(K.F.tiny)
                                    .foregroundStyle(on ? K.C.accent : K.C.faint)
                                VStack(alignment: .leading, spacing: K.S.hair) {
                                    Text(o.label).font(K.F.small.weight(on ? .semibold : .regular))
                                        .foregroundStyle(K.C.text)
                                    // What choosing it means. It was parsed off the tool input and
                                    // thrown away, leaving three one-word options to guess between.
                                    if !o.detail.isEmpty {
                                        Text(o.detail).font(K.F.tiny).foregroundStyle(K.C.dim)
                                            .fixedSize(horizontal: false, vertical: true)
                                    }
                                }
                                Spacer(minLength: K.S.sm)
                                // The first question's options answer to ⌘⌥1…9, and say so.
                                if questions.first?.id == q.id, i < 9 {
                                    Text("⌘⌥\(i + 1)").font(K.F.codeTiny).foregroundStyle(K.C.faint)
                                }
                            }
                            .padding(.vertical, K.S.tight)
                            .padding(.horizontal, K.S.sm)
                            .background(on ? K.C.accent.wash : .clear,
                                        in: RoundedRectangle(cornerRadius: K.R.sm))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .modifier(OptionKey(index: i, enabled: questions.first?.id == q.id))
                    }
                    TextField("Or answer in your own words…",
                              text: Binding(get: { other[q.id] ?? "" }, set: { other[q.id] = $0 }))
                        .textFieldStyle(.plain)
                        .font(K.F.small)
                        .padding(K.S.xs)
                        .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                }
            }

            HStack {
                Spacer()
                Button("Answer  ⌘⌥↩") { model.answer(pending, text: rendered) }
                    .buttonStyle(FilledButton())
                    .keyboardShortcut(.return, modifiers: [.command, .option])
                    .disabled(!complete)
            }
        }
        // The same card as an approval, because they are the same event: the turn has stopped and
        // it is waiting on you.
        .padding(K.S.md)
        .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.lg))
        .overlay(
            RoundedRectangle(cornerRadius: K.R.lg).stroke(K.C.warn.opacity(0.45), lineWidth: 1)
        )
        // The arrival is the parent's `.transition` with `K.M.enter`, not a `@State` flag flipped
        // in `onAppear`. That version started at `opacity(0)` and depended on `onAppear` firing to
        // become visible at all — which it does not in an offscreen render, and which made the one
        // card in the app that must never be missed the one card that could fail to draw.
        .shadow(color: .black.opacity(0.18), radius: 12, y: 3)
    }

    /// The answers as the text the agent reads: one line per question.
    private var rendered: String {
        questions.map { q in
            var parts = Array(chosen[q.id] ?? []).sorted()
            let typed = (other[q.id] ?? "").trimmingCharacters(in: .whitespaces)
            if !typed.isEmpty { parts.append(typed) }
            return "\(q.text): \(parts.joined(separator: ", "))"
        }.joined(separator: "\n")
    }
}

/// ⌘⌥n picks the nth option — on the first question only, so the digits mean one thing.
private struct OptionKey: ViewModifier {
    let index: Int
    let enabled: Bool
    func body(content: Content) -> some View {
        if enabled, index < 9 {
            content.keyboardShortcut(KeyEquivalent(Character("\(index + 1)")), modifiers: [.command, .option])
        } else {
            content
        }
    }
}


// MARK: - Monitoring

/// "Do you want me to watch this?"
///
/// A different question from a permission, which is why it is a different card. The agent is not
/// asking to be allowed to run something — `Bash` may already be allowed — it is asking for a
/// command to outlive the turn, and only Keel can grant that: a turn is one `claude -p`, and the
/// CLI kills its own background shells at teardown. Yes hands the command to the daemon, which
/// runs it and delivers the output back into this conversation when it finishes.
struct MonitorCard: View {
    let pending: Wire.Pending
    let model: SessionModel

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "binoculars.fill")
                    .font(K.F.micro).foregroundStyle(K.C.warn)
                    .accessibilityHidden(true)
                Text("Keep watching this after the turn ends?")
                    .font(K.F.body.weight(.semibold)).foregroundStyle(K.C.text)
                Spacer(minLength: 0)
            }

            Text(pending.command)
                .font(K.F.code)
                .foregroundStyle(K.C.text)
                .textSelection(.enabled)
                .lineLimit(24)
                .fixedSize(horizontal: false, vertical: true)
                .padding(K.S.sm)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))

            // Both outcomes named, because "No" here does not mean "don't run it" — it means
            // the agent runs it in the turn and waits for it. A card whose refusal is ambiguous
            // is one people answer wrong.
            HStack(spacing: K.S.sm) {
                Button("Monitor it") { model.answer(pending, allow: true, scope: "session") }
                    .buttonStyle(FilledButton())
                    .help("Keel runs it, it survives this turn, and its output comes back to this "
                          + "conversation when it finishes. Listed under Monitors.")
                Spacer()
                Button("Run it in this turn") { model.answer(pending, allow: false, scope: "session") }
                    .buttonStyle(QuietButton())
                    .help("The agent runs it in the foreground and reports the result before the "
                          + "turn ends")
            }
        }
        .padding(K.S.md)
        .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.lg))
        .overlay(
            RoundedRectangle(cornerRadius: K.R.lg).stroke(K.C.warn.opacity(0.45), lineWidth: 1)
        )
        .shadow(color: .black.opacity(0.18), radius: 12, y: 3)
    }
}
