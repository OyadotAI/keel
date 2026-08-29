import SwiftUI

/// The centre of a window: what the agent just did, reviewed like a pull request.
struct TurnStage: View {
    @Bindable var model: SessionModel

    /// Whether the pane is following the live turn. Scrolling up releases it.
    @State private var pinned = true

    /// What the pane lists. This is a record of what changed, so a turn that changed nothing has
    /// no entry — and a run of them is one line rather than five empty cards, which is what turned
    /// a scroll through a long session into scrolling past nothing.
    struct Row: Identifiable {
        enum Kind {
            case turn(Turn, Int)
            case quiet(Int, Int)
        }
        let id: String
        let kind: Kind
    }

    /// Everything that means "there is more to see at the bottom", as one value.
    private var tailToken: String {
        let last = model.turns.last
        return "\(model.turns.count)-\(last?.calls.count ?? 0)-\(last?.files.count ?? 0)-\(model.running)"
    }

    /// Keep the live turn in view.
    ///
    /// Scrolls to the *row* id, which is not always the turn's: a run of quiet turns collapses
    /// into one line, and scrolling to a turn that has no card is scrolling to nothing — which is
    /// why the pane sometimes simply did not move.
    ///
    /// Deliberately not animated. This is a tail following a live stream, like a terminal, and an
    /// easing curve restarted every few hundred milliseconds is what "flaky" looks like. The
    /// explicit jump from the conversation still animates, because that one is a deliberate move.
    private func follow(_ proxy: ScrollViewProxy) {
        // Only when nothing is deliberately focused: yanking the view while someone reads an
        // older turn is how a streaming pane becomes unusable.
        guard pinned, model.focusedTurn == nil, let id = rows.last?.id else { return }
        proxy.scrollTo(id, anchor: .bottom)
    }

    private var rows: [Row] {
        var out: [Row] = []
        var quietFrom: Int?

        func closeQuiet(_ upTo: Int) {
            guard let from = quietFrom else { return }
            out.append(Row(id: "quiet-\(from)-\(upTo)", kind: .quiet(from, upTo)))
            quietFrom = nil
        }

        for (i, turn) in model.turns.enumerated() {
            let n = i + 1
            // A live turn always gets a card: you are watching it, and it has not had the chance
            // to do anything yet.
            if turn.didWork || !turn.finished || turn.design != nil {
                closeQuiet(n - 1)
                out.append(Row(id: turn.id.uuidString, kind: .turn(turn, n)))
            } else {
                if quietFrom == nil { quietFrom = n }
            }
        }
        closeQuiet(model.turns.count)
        return out
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if let undo = model.undoSnapshot {
                        UndoBar(model: model, undo: undo)
                    }
                    if model.turns.isEmpty {
                        EmptyStage(model: model)
                    }
                    ForEach(rows) { row in
                        switch row.kind {
                        case .turn(let t, let n):
                            TurnCard(turn: t, number: n, model: model).id(row.id)
                        case .quiet(let first, let last):
                            QuietRun(first: first, last: last).id(row.id)
                        }
                        if row.id != rows.last?.id { Hairline() }
                    }
                }
            }
            .background(K.C.bg)
            // One handler on a compound token, not four on separate counts. Four meant a tool
            // call that also wrote a file fired two overlapping scroll animations, and the pane
            // visibly fought itself.
            .onChange(of: tailToken) { follow(proxy) }
            .onChange(of: model.pinTick) {
                pinned = true
                if let id = rows.last?.id { proxy.scrollTo(id, anchor: .bottom) }
            }
            .onScrollGeometryChange(for: Bool.self) { g in
                g.contentOffset.y + g.containerSize.height >= g.contentSize.height - 24
            } action: { was, atBottom in
                if was != atBottom { pinned = atBottom }
            }
            .overlay(alignment: .bottom) {
                if !pinned && model.running {
                    Button {
                        pinned = true
                        if let id = rows.last?.id {
                            withAnimation(K.M.settle) { proxy.scrollTo(id, anchor: .bottom) }
                        }
                    } label: {
                        HStack(spacing: 5) {
                            Image(systemName: "arrow.down").font(.system(size: 10, weight: .bold))
                            Text("Jump to latest").font(K.F.micro)
                        }
                        .padding(.horizontal, K.S.sm).padding(.vertical, 5)
                        .background(K.C.raised, in: Capsule())
                        .overlay(Capsule().stroke(K.C.lineStrong, lineWidth: 1))
                        .shadow(color: .black.opacity(0.25), radius: 8, y: 2)
                        .contentShape(Capsule())
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(K.C.text)
                    .padding(.bottom, K.S.md)
                }
            }
            .onChange(of: model.focusedTurn) {
                guard let id = model.focusedTurn else { return }
                withAnimation(K.M.settle) { proxy.scrollTo(id.uuidString, anchor: .top) }
            }
        }
    }
}

/// A run of turns that changed nothing, in one line.
private struct QuietRun: View {
    let first: Int
    let last: Int

    var body: some View {
        HStack(spacing: K.S.sm) {
            Text(first == last ? "Turn \(first)" : "Turns \(first)–\(last)")
                .font(.system(size: 10, weight: .semibold)).tracking(0.7)
            Text("answered in the conversation, changed nothing")
                .font(K.F.micro)
            Spacer()
        }
        .foregroundStyle(K.C.faint)
        .padding(.horizontal, K.S.xl).padding(.vertical, K.S.sm)
    }
}

private struct EmptyStage: View {
    let model: SessionModel

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            Text("Nothing has run yet")
                .font(K.F.title).foregroundStyle(K.C.text)
            Text("Ask for a change. Every file it writes and every command it runs appears here, "
                 + "and the project's own checks run when it says it is done.")
                .font(K.F.body).foregroundStyle(K.C.dim)
                .fixedSize(horizontal: false, vertical: true)

            HStack(spacing: K.S.sm) {
                if let gate = model.gateCommand {
                    Pill(text: "GATE", tone: .accent)
                    Text(gate).font(K.F.code).foregroundStyle(K.C.dim)
                } else {
                    Pill(text: "NO GATE", tone: .warn)
                    Text("this project declares no checks, so \"done\" is only a claim")
                        .font(K.F.small).foregroundStyle(K.C.dim)
                }
            }
            .padding(.top, K.S.xs)
        }
        .frame(maxWidth: 520, alignment: .leading)
        .padding(K.S.xl)
    }
}

// MARK: - One turn

/// One turn, as a record of what it did to the repository.
///
/// Deliberately not what it *said*: the prose reply lives in the conversation rail beside this,
/// and showing it twice is what made the window unreadable. This pane answers one question —
/// what changed and what ran — and answers it closed, because the shape of a turn is the count,
/// not the contents. You open what you want to read.
struct TurnCard: View {
    let turn: Turn
    let number: Int
    let model: SessionModel

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            header

            if !turn.files.isEmpty { ChangedFiles(turn: turn, model: model) }
            if !turn.calls.isEmpty { CommandList(turn: turn) }

            if turn.truncated {
                Text("This replay is capped at the most recent 300 calls — the session ran more.")
                    .font(K.F.micro).foregroundStyle(K.C.faint)
            }

            if let design = turn.design { DesignStrip(design: design) }

            if case .failed(_, let problems) = turn.gate, !problems.isEmpty {
                ProblemList(problems: problems, model: model)
            }

            ForEach(model.pending) { p in
                if p.isQuestion {
                    QuestionCard(pending: p, model: model)
                } else {
                    ApprovalCard(pending: p, model: model)
                }
            }

            TurnFooter(turn: turn)
        }
        .padding(.horizontal, K.S.xl)
        .padding(.vertical, K.S.lg)
        .background(
            model.focusedTurn == turn.id ? K.C.accent.opacity(0.05) : .clear
        )
    }

    /// Which turn, and when. Not what was asked — that is the conversation in the middle of the
    /// window, and printing it here again was the duplication between the two panes.
    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: K.S.sm) {
            Text("TURN \(number)")
                .font(.system(size: 10, weight: .semibold)).tracking(0.7)
                .foregroundStyle(K.C.faint)
            if !turn.finished { Pill(text: "RUNNING", tone: .accent) }
            Spacer(minLength: K.S.sm)
            Text(turn.started, style: .time)
                .font(K.F.mono(10)).foregroundStyle(K.C.faint)
            if turn.snapshot != nil, turn.finished {
                Menu {
                    Button("Restore files to before this turn") {
                        Task { await model.restore(to: turn) }
                    }
                } label: {
                    Image(systemName: "clock.arrow.circlepath").font(.system(size: 10))
                }
                .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                .foregroundStyle(K.C.faint)
                .hint("Rewind every file to how it was before this turn. Undoable.")
            }
        }
    }
}

/// After a rewind: the way back, for as long as it is one click away.
private struct UndoBar: View {
    let model: SessionModel
    let undo: String

    var body: some View {
        HStack(spacing: K.S.sm) {
            Image(systemName: "clock.arrow.circlepath").font(.system(size: 10))
                .foregroundStyle(K.C.accent)
            Text("Files were rewound.").font(K.F.small).foregroundStyle(K.C.text)
            Button("Undo") { Task { await model.restore(tree: undo); model.undoSnapshot = nil } }
                .buttonStyle(QuietButton(tone: K.C.accent))
            Spacer()
            CloseButton(size: 10, label: "Dismiss") { model.undoSnapshot = nil }
        }
        .padding(.horizontal, K.S.xl).padding(.vertical, K.S.sm)
        .background(K.C.accent.opacity(0.07))
    }
}

/// The files a turn touched. Closed: a list of names, with the stats that say whether any of them
/// is worth opening.
struct ChangedFiles: View {
    let turn: Turn
    let model: SessionModel
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SectionBar(
                title: "\(turn.files.count) file\(turn.files.count == 1 ? "" : "s") changed",
                open: $open
            )
            if open {
                VStack(alignment: .leading, spacing: K.S.sm) {
                    ForEach(turn.files, id: \.self) { path in
                        FileDiff(path: path, model: model)
                    }
                }
                .padding(.horizontal, K.S.sm)
                .padding(.bottom, K.S.sm)
            }
        }
        .background(K.C.surface, in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))
    }
}

/// The header that opens a section. One shape for both, so the two rows read as a pair.
struct SectionBar: View {
    let title: String
    @Binding var open: Bool
    var accessory: AnyView?
    @State private var hovering = false

    var body: some View {
        HStack(spacing: K.S.sm) {
            Image(systemName: open ? "chevron.down" : "chevron.right")
                .font(.system(size: 10, weight: .bold))
                .foregroundStyle(hovering ? K.C.dim : K.C.faint.opacity(0.6))
                .frame(width: 10)
            Text(title)
                .font(.system(size: 10, weight: .semibold)).tracking(0.4)
                .foregroundStyle(K.C.dim)
            if let accessory { accessory }
            Spacer()
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .background(hovering ? K.C.text.opacity(0.03) : .clear)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .asButton { withAnimation(K.M.quick) { open.toggle() } }
        .accessibilityLabel("\(title), \(open ? "expanded" : "collapsed")")
    }
}

extension View {
    /// Make a hand-drawn row a real control. A tap gesture on a view is invisible to the keyboard
    /// and to VoiceOver; wrapping the same view in a plain `Button` changes nothing on screen.
    func asButton(_ action: @escaping () -> Void) -> some View {
        Button(action: action) { self }.buttonStyle(.plain)
    }
}

// MARK: - Commands

/// Tool calls, one line each, with consecutive repeats collapsed. A turn that reads twenty files
/// is one row, not twenty — the twenty are available, they are just not the point.
private struct CommandList: View {
    let turn: Turn

    /// Set only when someone clicks the header. `nil` means "follow the turn", which is what makes
    /// a running turn show its work and a finished one fold back out of the way.
    @State private var userSet: Bool?
    @State private var showAll = false

    /// Open while the turn is running, closed once it is done — unless you said otherwise.
    ///
    /// Closed-by-default is right for reading back a finished turn: the shape is the count, and
    /// twenty rows of `Bash` is not information. It is exactly wrong while the agent is working,
    /// when the rows *are* the information and the only thing telling you it has not hung.
    private var open: Bool { userSet ?? !turn.finished }

    /// Fifteen rows is enough to see what kind of work happened once it is open; the rest are one
    /// more click away and almost never wanted.
    private static let visible = 15

    private var failed: Int { turn.calls.count { $0.failed } }

    private var accessory: AnyView? {
        if failed > 0 {
            return AnyView(Pill(text: "\(failed) FAILED", tone: .bad))
        }
        if !turn.finished {
            return AnyView(Pill(text: "RUNNING", tone: .accent))
        }
        return nil
    }

    var body: some View {
        let groups = turn.groups
        // While it runs, the newest work is the point, so the cap keeps the tail. Once it is
        // finished you are reading from the top, so the cap keeps the head.
        let shown = showAll ? groups
            : (turn.finished ? Array(groups.prefix(Self.visible))
                             : Array(groups.suffix(Self.visible)))

        VStack(alignment: .leading, spacing: 0) {
            SectionBar(
                title: "\(turn.calls.count) command\(turn.calls.count == 1 ? "" : "s")",
                open: Binding(get: { open }, set: { userSet = $0 }),
                accessory: accessory
            )

            if open {
                ForEach(shown) { group in
                    GroupRow(group: group,
                             live: !turn.finished && group.id == groups.last?.id)
                }

                if groups.count > Self.visible {
                    Button(showAll
                           ? "Show fewer"
                           : (turn.finished
                              ? "Show \(groups.count - Self.visible) more"
                              : "Show \(groups.count - Self.visible) earlier")) {
                        withAnimation(K.M.quick) { showAll.toggle() }
                    }
                    .buttonStyle(.plain)
                    .font(K.F.micro).foregroundStyle(K.C.accent)
                    .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
                    .contentShape(Rectangle())
                }
            }
        }
        .padding(.bottom, open ? K.S.xs : 0)
        .background(K.C.surface, in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))
    }
}

/// One run of calls to the same tool. Collapsed to a single line; expanding shows each call and
/// what it printed.
struct GroupRow: View {
    let group: Turn.Group
    /// The newest group of a turn still in flight — the one whose work is happening now.
    var live = false
    @State private var userSet: Bool?
    @State private var hovering = false

    /// Open while this is the live group, and only then.
    ///
    /// This used to key off `group.running`, which flipped every time a call finished and the next
    /// began — so a section popped open and shut on every command and the pane looked broken.
    /// Anchoring it to "newest group of a running turn" means one section is open for the duration
    /// and the rest stay put.
    private var open: Bool { userSet ?? live }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: K.S.sm) {
                StatusDot(failed: group.failed, running: group.running)
                Text(group.tool)
                    .font(K.F.mono(11, .medium))
                    .foregroundStyle(group.failed ? K.C.del : K.C.dim)
                    .frame(width: 56, alignment: .leading)

                if group.calls.count == 1 {
                    Subject(group.first.subject)
                } else {
                    Text("\(group.calls.count)×")
                        .font(K.F.mono(10, .medium)).foregroundStyle(K.C.faint)
                    Subject(group.first.subject)
                }
                // A subagent's work, as a count on its row rather than forty rows of its own.
                let nested = group.calls.reduce(0) { $0 + $1.children.count }
                if nested > 0 {
                    Text("\(nested) call\(nested == 1 ? "" : "s")")
                        .font(K.F.mono(10)).foregroundStyle(K.C.faint)
                }

                Spacer(minLength: K.S.sm)
                Image(systemName: open ? "chevron.down" : "chevron.right")
                    .font(.system(size: 10, weight: .bold))
                    .foregroundStyle(hovering ? K.C.dim : K.C.faint.opacity(0.45))
            }
            .padding(.horizontal, K.S.md).padding(.vertical, 3)
            .background(hovering ? K.C.text.opacity(0.04) : .clear)
            .contentShape(Rectangle())
            .onHover { hovering = $0 }
            .asButton { withAnimation(K.M.quick) { userSet = !open } }

            if open {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(group.calls) { call in
                        CallDetail(call: call,
                                   live: live && call.id == group.calls.last?.id)
                    }
                }
                .padding(.leading, K.S.xl)
                .padding(.bottom, K.S.xs)
            }
        }
    }
}

/// A command or a path, truncated at the *head*.
///
/// Middle truncation eats the filename, which is the part you were reading for — `…ling/api.py`
/// tells you nothing that `/Users/mk/Dev/o…` did not.
struct Subject: View {
    let text: String
    init(_ t: String) { text = t }

    var body: some View {
        Text(text)
            .font(K.F.code)
            .foregroundStyle(K.C.faint)
            .lineLimit(1)
            .truncationMode(.head)
            .frame(maxWidth: .infinity, alignment: .leading)
            .help(text)
    }
}

private struct CallDetail: View {
    let call: Turn.Call
    /// The newest call of the live group.
    var live = false
    @State private var userSet: Bool?

    /// Only the newest call shows its output unasked. Keying this off `call.running` meant every
    /// call in a run opened and closed in turn, which is the same churn one level down. A call
    /// with children is open while it is live, because the children are what is happening.
    private var open: Bool { userSet ?? (live && (!call.output.isEmpty || !call.children.isEmpty)) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: K.S.sm) {
                StatusDot(failed: call.failed, running: call.running)
                Subject(call.subject)
            }
            .padding(.vertical, 2).padding(.trailing, K.S.md)
            .contentShape(Rectangle())
            .asButton {
                if !call.output.isEmpty || !call.children.isEmpty {
                    withAnimation(K.M.quick) { userSet = !open }
                }
            }

            // What the subagent did, indented under the call that started it. The live child
            // is the newest running one, the same rule as one level up.
            if open, !call.children.isEmpty {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(call.children) { child in
                        CallDetail(call: child,
                                   live: live && child.running
                                       && child.id == call.children.last(where: \.running)?.id)
                    }
                }
                .padding(.leading, K.S.lg)
            }

            if open, !call.output.isEmpty {
                ScrollView {
                    Text(call.output.prefix(20_000))
                        .font(K.F.codeSmall).foregroundStyle(K.C.dim)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(K.S.sm)
                }
                .frame(maxHeight: 240)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                .padding(.trailing, K.S.md).padding(.bottom, K.S.xs)
            }
        }
    }
}

/// One dot, three meanings. Running pulses, so a stalled turn is visibly stalled.
struct StatusDot: View {
    let failed: Bool
    let running: Bool
    @Environment(\.accessibilityReduceMotion) private var still

    var body: some View {
        let color = failed ? K.C.del : (running ? K.C.accent : K.C.faint.opacity(0.55))
        // `PhaseAnimator` rather than an `onAppear` flag. The flag version restarted its cycle
        // every time the row was rebuilt — which, in a list that rebuilds on every streamed tool
        // call, was most frames — so a column of dots flickered out of step with each other.
        if running && !still {
            PhaseAnimator([1.0, 0.3]) { phase in
                Circle().fill(color).frame(width: 5, height: 5).opacity(phase)
            } animation: { _ in
                .easeInOut(duration: 0.7)
            }
        } else {
            Circle().fill(color).frame(width: 5, height: 5)
        }
    }
}

// MARK: - Footer

/// The verdict, the clock and the cost. `.passed` is the exit code of the project's own check,
/// run whether or not the agent bothered — not the agent's opinion of its own work.
struct TurnFooter: View {
    let turn: Turn

    /// Whether the footer has anything to say at all.
    private var speaks: Bool {
        // Nothing to report while it runs: the working bar above says what is happening, and an
        // empty footer row under every live turn is just a gap.
        if !turn.finished { return false }
        if turn.durationMS != nil || turn.cost != nil { return true }
        if case .notRun = turn.gate { return !turn.replayed && turn.didWork }
        return true
    }

    var body: some View {
        if speaks {
            row
        }
    }

    private var row: some View {
        HStack(spacing: K.S.md) {
            verdict
            Spacer()
            if let ms = turn.durationMS {
                Label(duration(ms), systemImage: "clock")
                    .labelStyle(.titleAndIcon)
                    .font(K.F.mono(10)).foregroundStyle(K.C.faint)
            }
            if let t = turn.tokens {
                Text("\(compact(t.input + t.cacheRead + t.cacheWrite)) in · \(compact(t.output)) out"
                     + (t.cacheRead > 0 ? String(format: " · %.0f%% cached", t.cached * 100) : ""))
                    .font(K.F.mono(10)).monospacedDigit().foregroundStyle(K.C.faint)
                    .help("Tokens this turn, as the CLI reported them")
            }
            if let c = turn.cost {
                Text(String(format: "$%.3f", c))
                    .font(K.F.mono(10)).monospacedDigit().foregroundStyle(K.C.faint)
            }
        }
        .padding(.top, K.S.xs)
    }

    @ViewBuilder
    private var verdict: some View {
        switch turn.gate {
        case .notRun:
            if !turn.finished {
                EmptyView()
            } else if turn.replayed || !turn.didWork {
                // Nothing true to say: Keel was not watching when a replayed turn ran, and a turn
                // that changed nothing has no claim for a gate to check.
                EmptyView()
            } else {
                Pill(text: "NO CHECKS", tone: .neutral)
            }
        case .running(let cmd):
            HStack(spacing: 5) {
                Pill(text: "CHECKING", tone: .accent)
                Text(cmd).font(K.F.mono(10)).foregroundStyle(K.C.faint)
            }
        case .passed(let cmd, let t):
            HStack(spacing: 5) {
                Pill(text: "GATE PASSED", tone: .good)
                Text(cmd).font(K.F.mono(10)).foregroundStyle(K.C.faint)
                Text(String(format: "%.1fs", t)).font(K.F.mono(10)).foregroundStyle(K.C.faint)
            }
        case .failed(let cmd, let problems):
            HStack(spacing: 5) {
                Pill(text: "GATE FAILED", tone: .bad)
                Text(cmd).font(K.F.mono(10)).foregroundStyle(K.C.faint)
                if !problems.isEmpty {
                    Text("\(problems.count) problem\(problems.count == 1 ? "" : "s")")
                        .font(K.F.mono(10)).foregroundStyle(K.C.del)
                }
            }
        case .none:
            // Why there is no gate is a fact about the project, not about this turn, and the
            // status bar says it once. Repeating it under every turn was most of the noise.
            Pill(text: "NO GATE", tone: .neutral)
        }
    }

    private func duration(_ ms: Int) -> String {
        let s = Double(ms) / 1000
        return s < 60 ? String(format: "%.1fs", s)
                      : String(format: "%dm%02ds", Int(s) / 60, Int(s) % 60)
    }
}

// MARK: - Problems

/// What the gate said went wrong, as places rather than a wall of output.
struct ProblemList: View {
    let problems: [Wire.Problem]
    let model: SessionModel

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            HStack {
                Text("PROBLEMS").font(.system(size: 10, weight: .semibold)).tracking(0.7)
                    .foregroundStyle(K.C.del)
                Spacer()
                Button("Hand these to the agent") { model.prompt = prompt }
                    .buttonStyle(QuietButton())
            }
            .padding(.bottom, 2)

            ForEach(problems.prefix(20)) { p in
                HStack(alignment: .firstTextBaseline, spacing: K.S.sm) {
                    Text(p.severity == "error" ? "✗" : "!")
                        .font(K.F.mono(10, .bold))
                        .foregroundStyle(p.severity == "error" ? K.C.del : K.C.warn)
                        .frame(width: 10)
                    Text(p.message).font(K.F.small).foregroundStyle(K.C.text)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: K.S.sm)
                    Text("\(p.file):\(p.line)")
                        .font(K.F.mono(10)).foregroundStyle(K.C.faint)
                        .lineLimit(1).truncationMode(.head)
                }
            }
            if problems.count > 20 {
                Text("and \(problems.count - 20) more")
                    .font(K.F.micro).foregroundStyle(K.C.faint)
            }
        }
        .padding(K.S.md)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(K.C.del.opacity(0.06), in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.del.opacity(0.25), lineWidth: 1))
    }

    private var prompt: String {
        "The project's checks failed. Fix these:\n\n"
            + problems.prefix(30)
                .map { "\($0.file):\($0.line):\($0.col) \($0.severity): \($0.message)" }
                .joined(separator: "\n")
    }
}

/// A button that reads as a control without shouting. Stock `.bordered` is too heavy for a dense
/// surface, and `.plain` gives no affordance at all.
struct QuietButton: ButtonStyle {
    var tone: Color = K.C.dim
    @State private var hovering = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(K.F.small)
            .foregroundStyle(configuration.isPressed ? K.C.text : tone)
            .padding(.horizontal, K.S.sm).padding(.vertical, 3)
            .background(
                RoundedRectangle(cornerRadius: K.R.sm)
                    .fill(hovering ? K.C.text.opacity(0.07) : K.C.text.opacity(0.03))
            )
            .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
            .onHover { hovering = $0 }
    }
}
