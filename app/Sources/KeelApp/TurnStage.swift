import AppKit
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
            /// First and last turn number, and the prompt when it is a single turn — a row that
            /// says only "changed nothing" does not say what was asked.
            case quiet(Int, Int, String)
        }
        let id: String
        let kind: Kind
    }

    /// The last time the pane followed, so a stream of deltas is not a stream of scrolls.
    @State private var lastFollow = Date.distantPast

    /// How many rows the pane shows before it offers the rest.
    ///
    /// The Trace answers "what did it just do", and a session has hundreds of turns behind that
    /// question — so the newest is at the top and the older ones are a click away rather than a
    /// scroll away. Five is what fits on a screen without becoming a list to search.
    private static let shown = 5
    @State private var showingAll = false

    /// Keep the live turn in view.
    ///
    /// Scrolls to the *row* id, which is not always the turn's: a run of quiet turns collapses
    /// into one line, and scrolling to a turn that has no card is scrolling to nothing — which is
    /// why the pane sometimes simply did not move.
    ///
    /// Deliberately not animated. This is a tail following a live stream, like a terminal, and an
    /// easing curve restarted every few hundred milliseconds is what "flaky" looks like. The
    /// explicit jump from the conversation still animates, because that one is a deliberate move.
    ///
    /// Coalesced at 80ms, the same window the conversation uses: a running command prints faster
    /// than a frame, and a scroll on every delta competes with the wheel. `force` is for the end
    /// of a turn, the one update that has no next one to correct a short scroll.
    private func follow(_ proxy: ScrollViewProxy, force: Bool = false) {
        // Only when nothing is deliberately focused: yanking the view while someone reads an
        // older turn is how a streaming pane becomes unusable.
        guard pinned, model.focusedTurn == nil, let id = rows.first?.id else { return }
        let now = Date()
        guard force || now.timeIntervalSince(lastFollow) > 0.08 else { return }
        lastFollow = now
        proxy.scrollTo(id, anchor: .top)
    }

    /// The row a turn is drawn in — its own card, or the quiet run it was folded into.
    private func row(holding turn: UUID) -> String? {
        guard let n = model.turns.firstIndex(where: { $0.id == turn }).map({ $0 + 1 }) else { return nil }
        return rows.first { row in
            switch row.kind {
            case .turn(let t, _): t.id == turn
            case .quiet(let first, let last, _): n >= first && n <= last
            }
        }?.id
    }

    private var rows: [Row] {
        var out: [Row] = []
        var quietFrom: Int?
        var quietPrompt = ""

        func closeQuiet(_ upTo: Int) {
            guard let from = quietFrom else { return }
            // Keyed on where the run starts, not on where it currently ends: `quiet-3-3` becoming
            // `quiet-3-4` and then `quiet-3-5` as turns land is a new row each time, so the one
            // being drawn is torn down and rebuilt on every quiet turn.
            out.append(Row(id: "quiet-\(from)",
                           kind: .quiet(from, upTo, from == upTo ? quietPrompt : "")))
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
                if quietFrom == nil { quietFrom = n; quietPrompt = turn.prompt }
            }
        }
        closeQuiet(model.turns.count)
        // Newest first. The turn you want is the one that just happened, and at the top it is
        // there without a scroll — which is also why the follow below holds the *first* row.
        return out.reversed()
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if let undo = model.undoSnapshot {
                        UndoBar(model: model, undo: undo)
                    }
                    // A pane with nothing in it says which of the reasons it is. This pane
                    // used to read none of them: it asserted "Nothing has run yet" over a
                    // session that was still being read, and for good over one that could not be.
                    if case .failed(let why) = model.replay {
                        ReplayFailed(why: why) {
                            Task { if let id = model.sessionId { await model.open(session: id) } }
                        }
                    }
                    if model.turns.isEmpty {
                        switch model.replay {
                        case .reading: OpeningNote().frame(maxWidth: .infinity).padding(K.S.xl)
                        case .failed: EmptyView()
                        case .none: EmptyStage(model: model)
                        }
                    }
                    // `rows` walks every turn, so it is read once here rather than once per
                    // row: `row.id != rows.last?.id` inside the loop made drawing the list
                    // quadratic in its own length.
                    let rows = rows
                    let shown = showingAll ? rows : Array(rows.prefix(Self.shown))
                    let lastRow = shown.last?.id
                    ForEach(shown) { row in
                        switch row.kind {
                        case .turn(let t, let n):
                            TurnCard(turn: t, number: n, model: model,
                                     newest: row.id == rows.first?.id).id(row.id)
                        case .quiet(let first, let last, let prompt):
                            QuietRun(first: first, last: last, prompt: prompt).id(row.id)
                        }
                        if row.id != lastRow { Hairline() }
                    }
                    if !showingAll, rows.count > Self.shown {
                        Hairline()
                        ShowMore(count: rows.count - Self.shown) {
                            withAnimation(K.M.flow) { showingAll = true }
                        }
                    }
                    // Oldest is at the bottom here, so what came before it is said there.
                    if let dropped = SessionModel.droppedNotice(model.replayDropped, bytes: model.replayDroppedBytes) {
                        Text(dropped).font(K.F.micro).foregroundStyle(K.C.faint)
                            .padding(.horizontal, K.S.xl).padding(.vertical, K.S.md)
                    }
                }
            }
            .background(K.C.bg)
            // One handler on a compound token, not four on separate counts. Four meant a tool
            // call that also wrote a file fired two overlapping scroll animations, and the pane
            // visibly fought itself.
            //
            // In a view of its own rather than here: read from this `body` the token's dependency
            // belongs to the whole pane, so every delta rebuilt `rows` over every turn and every
            // card in the list. See `TailFollower`.
            .overlay { TailFollower(model: model) { follow(proxy) } }
            .onChange(of: model.pinTick) {
                pinned = true
                showingAll = false
                if let id = rows.first?.id { proxy.scrollTo(id, anchor: .top) }
            }
            // The tail of this pane is its top: see `rows`.
            .followsTail($pinned, top: true)
            .onChange(of: model.running) { follow(proxy, force: true) }
            .overlay(alignment: .bottom) {
                if !pinned && model.running {
                    JumpToLatest {
                        pinned = true
                        // Going back to the tail is the gesture that says "stop holding me at
                        // that turn", and it is what releases `follow`.
                        model.focusedTurn = nil
                        if let id = rows.first?.id {
                            withAnimation(K.M.settle) { proxy.scrollTo(id, anchor: .top) }
                        }
                    }
                }
            }
            // Resolved through `rows`, and `initial: true` because the pane may not be mounted.
            //
            // A turn that changed nothing has no card — it is folded into a `QuietRun` — so
            // scrolling to its own id was a silent no-op and the "Turn N →" marker beside it did
            // nothing at all.
            //
            // `initial: true` is the other half: clicking the marker while the stage is showing
            // the preview or the review packet *mounts* this pane, and a plain `onChange` does
            // not fire on appear — so the jump never happened, and `focusedTurn` stayed set with
            // `follow` refusing to scroll for as long as it was. It is cleared by the next send
            // rather than here, so the wash that says which turn you landed on survives the frame.
            //
            // A turn below the fold has no card either, for the same reason and with the same
            // symptom: the marker points at an older turn, the pane shows the five newest, and
            // the jump lands on nothing. Asked for, the rest of the session opens.
            .onChange(of: model.focusedTurn, initial: true) {
                guard let id = model.focusedTurn, let row = row(holding: id) else { return }
                let drawn = (showingAll ? rows : Array(rows.prefix(Self.shown)))
                    .contains { $0.id == row }
                guard drawn else {
                    showingAll = true
                    // After the rows it just asked for exist. A `scrollTo` in the same frame
                    // resolves against a list that does not hold that row yet.
                    Task { @MainActor in
                        withAnimation(K.M.settle) { proxy.scrollTo(row, anchor: .top) }
                    }
                    return
                }
                withAnimation(K.M.settle) { proxy.scrollTo(row, anchor: .top) }
            }
        }
    }
}

/// The rest of the session, one click away.
///
/// A button rather than a scroll: the Trace is newest-first and the turn you came for is at the
/// top, so everything below the fold is history — and drawing four hundred cards to keep it
/// reachable is the thing that makes a long session slow to open.
struct ShowMore: View {
    let count: Int
    /// Which way the rest of the session lies: below in the Trace, above in the conversation.
    var up = false
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        HStack(spacing: K.S.sm) {
            Image(systemName: up ? "chevron.up" : "chevron.down").font(K.F.tiny.weight(.bold))
            Text("\(count) earlier turn\(count == 1 ? "" : "s")").font(K.F.small)
            Spacer()
        }
        .foregroundStyle(hovering ? K.C.text : K.C.dim)
        .padding(.horizontal, K.S.xl).padding(.vertical, K.S.md)
        .background(hovering ? K.C.hover : .clear)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .asButton(action)
        .accessibilityLabel("Show \(count) earlier turns")
    }
}

/// A run of turns that changed nothing, in one line.
private struct QuietRun: View {
    let first: Int
    let last: Int
    /// Empty for a run of several — one line cannot honestly stand for three different asks.
    var prompt = ""

    var body: some View {
        HStack(spacing: K.S.sm) {
            Text(first == last ? "Turn \(first)" : "Turns \(first)–\(last)")
                .sectionLabel()
            if !prompt.isEmpty {
                Text(prompt.capped(200)).font(K.F.micro).lineLimit(1).truncationMode(.tail)
                    .foregroundStyle(K.C.dim).help(prompt.capped(500))
            }
            Text("answered in the conversation, changed nothing")
                .font(K.F.micro).layoutPriority(1)
            Spacer()
        }
        .foregroundStyle(K.C.faint)
        .padding(.horizontal, K.S.xl).padding(.vertical, K.S.sm)
    }
}

private struct EmptyStage: View {
    let model: SessionModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            EmptyState(icon: "list.bullet.rectangle", title: "Nothing has run yet",
                       "Ask for a change. Every file it writes and every command it runs appears "
                       + "here, and the project's own checks run when it says it is done.")

            // The one empty state with more to say than a sentence: whether this project has a
            // gate at all changes what "done" is worth, and it is worth knowing before the turn.
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

/// One turn, as a record of what it did to the repository: the files it changed, and the stream
/// it produced while changing them.
///
/// Deliberately not what it *said*: the prose reply lives in the conversation rail beside this,
/// and showing it twice is what made the window unreadable. Deliberately not the command list
/// either, which is on screen twice already — inline in the conversation, where each call keeps
/// its input and its output, and folded in the Review pane, where it is evidence for the merge.
/// A third copy was the widest thing in this pane and the one nobody read.
struct TurnCard: View {
    let turn: Turn
    let number: Int
    let model: SessionModel
    /// The top card. The pane lists newest first, so this is the turn you opened the window to
    /// look at — its console is open without a click.
    var newest = false

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            header
            // Capped as a string, not only by `lineLimit` — two lines of a 12,000-character paste
            // still costs measuring all 12,000. `String.capped` says why.
            Text(turn.prompt.capped(300))
                .font(K.F.small).foregroundStyle(K.C.dim)
                .lineLimit(2).truncationMode(.tail)

            if !turn.files.isEmpty { ChangedFiles(turn: turn, model: model) }

            if let why = turn.notIsolated {
                // The turn was meant to be isolated and could not be. Said plainly, because it
                // changes where the agent wrote — but the work happened, which is the point.
                HStack(alignment: .top, spacing: K.S.sm) {
                    Image(systemName: "info.circle").font(K.F.tiny)
                    Text("Ran in the project rather than an isolated checkout — \(why)")
                        .fixedSize(horizontal: false, vertical: true)
                }
                .font(K.F.small).foregroundStyle(K.C.warn)
            }

            if let failure = turn.failure {
                HStack(alignment: .top, spacing: K.S.sm) {
                    Image(systemName: "exclamationmark.triangle.fill").font(K.F.tiny)
                    Text(failure).fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
                .font(K.F.small).foregroundStyle(K.C.del)
            }

            if let design = turn.design { DesignStrip(design: design) }

            if case .failed(_, let problems) = turn.gate, !problems.isEmpty {
                ProblemList(problems: problems, model: model)
            }

            // Questions and approvals live in the conversation pane, above the composer —
            // the one place on screen whatever the stage is showing. Rendering them here too
            // would register every shortcut twice.

            RawOutput(turn: turn, newest: newest)

            TurnFooter(turn: turn)
        }
        .padding(.horizontal, K.S.xl)
        .padding(.vertical, K.S.lg)
        .background(
            model.focusedTurn == turn.id ? K.C.accent.wash : .clear
        )
    }

    /// Which turn and when. The compact prompt beneath this header anchors execution evidence to
    /// intent without repeating the full conversation.
    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: K.S.sm) {
            Text("TURN \(number)")
                .sectionLabel()
                .foregroundStyle(K.C.faint)
            if !turn.finished {
                Pill(text: model.following ? "RUNNING · outside Keel" : "RUNNING", tone: .accent)
            }
            Spacer(minLength: K.S.sm)
            Text(turn.started, style: .time)
                .font(K.F.codeTiny).foregroundStyle(K.C.faint)
            if turn.snapshot != nil, turn.finished {
                Menu {
                    Button("Restore files to before this turn") {
                        Task { await model.restore(to: turn) }
                    }
                } label: {
                    Image(systemName: "clock.arrow.circlepath").font(K.F.tiny)
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
            Image(systemName: "clock.arrow.circlepath").font(K.F.tiny)
                .foregroundStyle(K.C.accent)
            Text("Files were rewound.").font(K.F.small).foregroundStyle(K.C.text)
            Button("Undo") { Task { await model.restore(tree: undo); model.undoSnapshot = nil } }
                .buttonStyle(QuietButton(tone: K.C.accent))
            Spacer()
            CloseButton(size: 10, label: "Dismiss") { model.undoSnapshot = nil }
        }
        .padding(.horizontal, K.S.xl).padding(.vertical, K.S.sm)
        .background(K.C.accent.wash)
    }
}

/// The files a turn touched. Closed: a list of names, with the stats that say whether any of them
/// is worth opening.
struct ChangedFiles: View {
    let turn: Turn
    let model: SessionModel
    /// Open. The files are the answer to "what did this turn do" — folding them costs a click on
    /// every turn to see the thing the pane is for. Each file's *diff* is still closed, so this
    /// is a list of names and stats, not a wall of code.
    @State private var open = true

    /// Counted in the header rather than left to be discovered by scrolling: a turn that wrote
    /// three files in the repository and one in `/tmp` did something the first number alone does
    /// not describe, and the one outside is the one nothing else in Keel will ever mention again.
    private var outside: Int {
        turn.files.count { model.repoRelative($0).hasPrefix("/") }
    }

    private var title: String {
        let n = turn.files.count
        let files = "\(n) file\(n == 1 ? "" : "s") changed"
        return outside == 0 ? files : "\(files) · \(outside) outside the repository"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SectionBar(title: title, open: $open)
            if open {
                VStack(alignment: .leading, spacing: K.S.sm) {
                    // Through the same normaliser the Changes panel uses: an absolute path from
                    // a lane's checkout is not a path this checkout's git can answer for.
                    ForEach(turn.files, id: \.self) { path in
                        let shown = model.repoRelative(path)
                        // Still absolute after the normaliser means it is not in this checkout at
                        // all, so there is no `git diff` to ask for and the answer would be an
                        // empty card. Say where it is and whose git has it instead.
                        if shown.hasPrefix("/") {
                            OutsideFile(path: shown, model: model)
                        } else {
                            FileDiff(path: shown, model: model)
                        }
                    }
                }
                .padding(.horizontal, K.S.sm)
                .padding(.bottom, K.S.sm)
                            .transition(.opacity)
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
                .font(K.F.tiny.weight(.bold))
                .foregroundStyle(hovering ? K.C.dim : K.C.faint.opacity(0.6))
                .frame(width: 10)
            Text(title)
                .sectionLabel()
                .foregroundStyle(K.C.dim)
            if let accessory { accessory }
            Spacer()
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .background(hovering ? K.C.hover : .clear)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .asButton { withAnimation(K.M.flow) { open.toggle() } }
        .accessibilityLabel("\(title), \(open ? "expanded" : "collapsed")")
    }
}

// MARK: - Shared row furniture

/// What a call could do, as one glyph: reads, changes something, or destroys something.
///
/// Failure wins over risk — a command that failed is a fact, where its risk was only ever a guess
/// — and a running call keeps the pulse, because that is what says the turn is alive.
struct CallGlyph: View {
    let risk: Turn.Risk
    let failed: Bool
    let running: Bool
    @Environment(\.accessibilityReduceMotion) private var still

    private var symbol: String {
        if failed { return "xmark.octagon.fill" }
        switch risk {
        case .safe: return "eye"
        case .warn: return "exclamationmark.triangle.fill"
        case .danger: return "exclamationmark.octagon.fill"
        }
    }

    private var color: Color {
        if failed { return K.C.del }
        switch risk {
        case .safe: return running ? K.C.accent : K.C.faint.opacity(0.7)
        case .warn: return K.C.warn
        case .danger: return K.C.del
        }
    }

    private var label: String {
        if failed { return "failed" }
        switch risk {
        case .safe: return "reads only"
        case .warn: return "changes something"
        case .danger: return "destructive"
        }
    }

    var body: some View {
        ZStack {
            // The pulse is the same one the dot had, drawn around the glyph rather than instead
            // of it: you should not have to give up knowing what it is to see that it is running.
            if running && !still {
                PhaseAnimator([false, true]) { expanded in
                    Circle().stroke(color.opacity(expanded ? 0 : 0.45), lineWidth: 1)
                        .frame(width: expanded ? 16 : 8, height: expanded ? 16 : 8)
                } animation: { _ in .easeOut(duration: 0.9) }
            }
            Image(systemName: symbol)
                .font(K.F.tiny.weight(.semibold))
                .foregroundStyle(color)
        }
        .frame(width: 16, height: 16)
        .hint(label)
    }
}

/// How long a call took, ticking while it is still running.
///
/// The list could say what ran and whether it worked, and nothing at all about time — so a command
/// that had been going for four minutes looked exactly like one that had just started, which is
/// the moment you most want to know the difference.
struct Elapsed: View {
    let started: Date
    var duration: TimeInterval?
    var running: Bool

    var body: some View {
        if running {
            // Ticks on its own. An `onAppear` timer would restart every time the list rebuilt,
            // which in a streaming turn is most frames.
            TimelineView(.periodic(from: .now, by: 1)) { context in
                text(context.date.timeIntervalSince(started), live: true)
            }
        } else if let duration, duration >= 1 {
            // Under a second is noise: forty rows of "0s" is a column that says nothing.
            text(duration, live: false)
        }
    }

    private func text(_ seconds: TimeInterval, live: Bool) -> some View {
        let s = max(0, Int(seconds))
        let label = s < 60 ? "\(s)s" : "\(s / 60)m\(String(format: "%02d", s % 60))s"
        return Text(label)
            .font(K.F.codeTiny).monospacedDigit()
            .foregroundStyle(live ? K.C.accent : K.C.faint.opacity(0.7))
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
            PhaseAnimator([false, true]) { expanded in
                ZStack {
                    Circle().stroke(color.opacity(expanded ? 0 : 0.45), lineWidth: 1)
                        .frame(width: expanded ? 13 : 6, height: expanded ? 13 : 6)
                    Circle().fill(color).frame(width: 6, height: 6)
                }
                .frame(width: 13, height: 13)
            } animation: { _ in
                .easeOut(duration: 0.9)
            }
        } else {
            Circle().fill(color).frame(width: 6, height: 6)
                .frame(width: 13, height: 13)
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

    /// A strip of labelled cells rather than a line of fragments: the caption says what the
    /// number is, the value stands alone, and the gate is the one cell with a colour.
    private var row: some View {
        HStack(spacing: 0) {
            gateCell
            if let sha = turn.commit {
                divider
                cell("COMMIT", sha, icon: "checkmark.circle", tone: K.C.add,
                     help: "Keel committed this turn's changes as \(sha). Undo from the Changes panel.")
            }
            Spacer(minLength: K.S.md)
            if let ms = turn.durationMS {
                cell("TIME", duration(ms), icon: "clock", help: "Wall-clock time for this turn")
                divider
            }
            if let t = turn.tokens {
                let value = "\(compact(t.input + t.cacheRead + t.cacheWrite)) in · \(compact(t.output)) out"
                cell("TOKENS", value, icon: "arrow.up.arrow.down",
                     help: "Tokens this turn, as the CLI reported them: \(t.input + t.cacheRead + t.cacheWrite) in, \(t.output) out"
                           + (t.cacheRead > 0 ? ", " + t.cached.formatted(.percent.precision(.fractionLength(0))) + " served from cache" : ""))
                if t.cacheRead > 0 {
                    Text(t.cached.formatted(.percent.precision(.fractionLength(0))) + " cached")
                        .font(K.F.micro).foregroundStyle(K.C.faint).padding(.leading, K.S.xs)
                        .padding(.trailing, K.S.sm)
                }
                if turn.cost != nil { divider }
            }
            if let c = turn.cost {
                cell("COST", money(c), icon: "dollarsign.circle", help: "What this turn cost, from the CLI's own figure")
            }
        }
        .padding(.top, K.S.sm)
    }

    private var divider: some View {
        Rectangle().fill(K.C.line).frame(width: 1, height: 22).padding(.horizontal, K.S.sm)
    }

    /// Caption over value. The caption is what made the old line unreadable by its absence.
    private func cell(_ label: String, _ value: String, icon: String, tone: Color = K.C.text, help: String) -> some View {
        VStack(alignment: .leading, spacing: K.S.hair) {
            Text(label).sectionLabel().foregroundStyle(K.C.faint)
            HStack(spacing: K.S.xs) {
                Image(systemName: icon).font(K.F.tiny).foregroundStyle(tone == K.C.text ? K.C.faint : tone)
                Text(value).font(K.F.codeSmall).monospacedDigit().foregroundStyle(tone).lineLimit(1)
            }
        }
        .help(help)
    }

    @ViewBuilder
    private var gateCell: some View {
        switch turn.gate {
        case .notRun:
            if turn.finished, !turn.replayed, turn.didWork {
                cell("GATE", "no checks ran", icon: "minus.circle", help: "This turn changed files but nothing was checked afterwards.")
            }
        case .running(let cmd):
            cell("GATE", "checking · \(short(cmd))", icon: "circle.dotted", tone: K.C.accent, help: "Running \(cmd)")
        case .passed(let cmd, let t):
            let took = t.formatted(.number.precision(.fractionLength(1)))
            cell("GATE", "passed · \(short(cmd)) · \(took)s", icon: "checkmark.seal.fill", tone: K.C.add,
                 help: "\(cmd) passed in \(took)s")
        case .failed(let cmd, let problems):
            cell("GATE", "failed · \(short(cmd))" + (problems.isEmpty ? "" : " · \(problems.count) problem\(problems.count == 1 ? "" : "s")"),
                 icon: "xmark.seal.fill", tone: K.C.del, help: "\(cmd) failed" + (problems.isEmpty ? "" : " with \(problems.count) problems, listed above"))
        case .none:
            // Why there is no gate is a fact about the project, not about this turn, and the
            // status bar says it once. Repeating it under every turn was most of the noise.
            cell("GATE", "none set", icon: "minus.circle", help: "No check command is configured for this project. Set one in the status bar.")
        }
    }

    /// `go test ./... && go vet ./...` → `go test ./…`: the first command, elided.
    private func short(_ cmd: String) -> String {
        let first = cmd.split(separator: "&&").first.map { $0.trimmingCharacters(in: .whitespaces) } ?? cmd
        return first.count > 22 ? String(first.prefix(21)) + "…" : first
    }

    private func duration(_ ms: Int) -> String {
        let s = Double(ms) / 1000
        return s < 60
            ? s.formatted(.number.precision(.fractionLength(1))) + "s"
            : "\(Int(s) / 60)m\((Int(s) % 60).formatted(.number.precision(.integerLength(2))))s"
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
                Text("PROBLEMS").sectionLabel()
                    .foregroundStyle(K.C.del)
                Spacer()
                Button("Hand these to the agent") { model.prompt = prompt }
                    .buttonStyle(QuietButton())
            }
            .padding(.bottom, K.S.xxs)

            ForEach(problems.prefix(20)) { p in
                HStack(alignment: .firstTextBaseline, spacing: K.S.sm) {
                    Text(p.severity == "error" ? "✗" : "!")
                        .font(K.F.codeTiny.weight(.bold))
                        .foregroundStyle(p.severity == "error" ? K.C.del : K.C.warn)
                        .frame(width: 10)
                    Text(p.message).font(K.F.small).foregroundStyle(K.C.text)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: K.S.sm)
                    Text("\(p.file):\(p.line)")
                        .font(K.F.codeTiny).foregroundStyle(K.C.faint)
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




/// Exactly what the provider printed, as a console — the worst-case answer to "what is it doing",
/// and now the pane's main event rather than a footnote under it.
///
/// The guarantee: there is no state in which Keel saw something and the person cannot. Everything
/// else in this app is an interpretation of these bytes, and every interpretation can be wrong or
/// missing.
///
/// Two things make it readable rather than merely present. It **tails** while the turn runs, like
/// the terminal it replaced, so the newest line is the one on screen — coalesced at 80ms, because
/// a provider prints faster than a frame and a scroll per line fights the wheel. And a record is
/// **indented**: the stream is one JSON object per line, and a four-thousand-character line is
/// something nobody reads. Anything that is not JSON — stderr, the exit line — is left alone.
struct RawOutput: View {
    let turn: Turn
    /// The newest turn in the pane. Its console opens on load, because a session you have just
    /// opened is one you are opening to see what happened — and the answer nothing can be wrong
    /// about is this one. Every older turn stays folded: five open consoles is a wall.
    var newest = false
    /// Set only by a click. `nil` follows the turn: open while it runs, folded once it is done,
    /// which is the same rule the command list used to have and the right one here — while it is
    /// running these lines are the only thing saying it has not hung.
    @State private var userSet: Bool?
    @State private var indented = true
    @State private var lastFollow = Date.distantPast
    @State private var copied = false

    private var open: Bool { userSet ?? (newest || !turn.finished) }

    /// The console as it is drawn, so what lands on the pasteboard is what you were looking at —
    /// including the indenting, and including the note about what was not kept.
    ///
    /// Static so it can be tested without a view: what makes this worth a test is the dropped
    /// count, and a silently short log is the failure it exists to prevent.
    static func text(of turn: Turn, indented: Bool) -> String {
        var out = turn.raw.map { indented ? $0.text.indentedJSON : $0.text }
        if turn.rawDropped > 0 {
            out.append("…and \(turn.rawDropped) more lines, not kept")
        }
        return out.joined(separator: "\n")
    }

    var body: some View {
        if !turn.raw.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                header
                if open { console.transition(.opacity) }
            }
            .animation(K.M.flow, value: open)
        }
    }

    private var header: some View {
        HStack(spacing: K.S.xs) {
            Button { withAnimation(K.M.flow) { userSet = !open } } label: {
                HStack(spacing: K.S.xs) {
                    Image(systemName: "chevron.right")
                        .font(K.F.ui(7, .bold))
                        .rotationEffect(.degrees(open ? 90 : 0))
                        .foregroundStyle(K.C.faint)
                    Text("CONSOLE").sectionLabel().foregroundStyle(K.C.faint)
                    Text("\(turn.raw.count) line\(turn.raw.count == 1 ? "" : "s")")
                        .font(K.F.codeTiny).foregroundStyle(K.C.faint)
                    // The two counts that say "Keel did not understand this" out loud.
                    if turn.unreadable > 0 {
                        Text("· \(turn.unreadable) unreadable")
                            .font(K.F.codeTiny).foregroundStyle(K.C.warn)
                    }
                    if !turn.unknown.isEmpty {
                        Text("· " + turn.unknown.sorted().joined(separator: ", "))
                            .font(K.F.codeTiny).foregroundStyle(K.C.faint)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Spacer(minLength: K.S.sm)
            if open {
                Button(indented ? "one line each" : "indent JSON") { indented.toggle() }
                    .buttonStyle(.plain)
                    .font(K.F.micro).foregroundStyle(K.C.accent)
                    .help(indented
                          ? "Show each record as the single line it arrived as"
                          : "Indent each record that is JSON")
                // The whole console, not the selection. This is the pane a person reaches for
                // when they are about to paste what happened into an issue, and dragging a
                // selection through two thousand lines of a scroll view is not that.
                CopyChip(label: "console", copied: copied) { copy() }
                    .help("Copy every line of the console")
            }
        }
        .padding(.vertical, K.S.xxs)
    }

    private var console: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(turn.raw) { line in
                        // Formatted in the row rather than up front: the stack is lazy, so only
                        // what is on screen is ever parsed. Two thousand records pretty-printed
                        // eagerly is the hang `String.capped` exists for.
                        Text(indented ? line.text.indentedJSON : line.text.capped(2_000))
                            .font(K.F.codeTiny)
                            .foregroundStyle(line.stream == .err ? K.C.del : K.C.dim)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.vertical, K.S.hair)
                            .id(line.id)
                    }
                    if turn.rawDropped > 0 {
                        Text("…and \(turn.rawDropped) more lines, not kept")
                            .font(K.F.micro).foregroundStyle(K.C.faint)
                    }
                }
                .padding(K.S.sm)
            }
            .frame(maxHeight: 300)
            .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
            .onChange(of: turn.raw.count, initial: true) { tail(proxy) }
        }
    }

    private func copy() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(Self.text(of: turn, indented: indented), forType: .string)
        copied = true
        Task { try? await Task.sleep(for: .seconds(1.4)); copied = false }
    }

    /// Follow the newest line while the turn is live. A finished turn is read from wherever you
    /// put it, so it is not dragged anywhere.
    private func tail(_ proxy: ScrollViewProxy) {
        guard !turn.finished, let id = turn.raw.last?.id else { return }
        let now = Date()
        guard now.timeIntervalSince(lastFollow) > 0.08 else { return }
        lastFollow = now
        proxy.scrollTo(id, anchor: .top)
    }
}

/// A file this turn wrote that git cannot answer for, because it is not in this checkout.
///
/// There is no diff to draw, and the reason the row exists is not the diff: it is that the file
/// was written at all. A turn that puts something in `/tmp`, in the home directory, or into
/// somebody else's repository and then says nothing about it is the invisible half of "see what
/// your agent actually did" — and invisible is the one thing Keel is for.
private struct OutsideFile: View {
    let path: String
    let model: SessionModel

    /// The repository holding this file, if any — resolved once, off the render path.
    @State private var repo: String??
    @State private var hovering = false

    var body: some View {
        HStack(spacing: K.S.sm) {
            Image(systemName: "arrow.up.forward.app")
                .font(K.F.tiny).foregroundStyle(K.C.faint).frame(width: 10)

            // Same reading order as a diff header: directory dimmed, filename not.
            HStack(spacing: 0) {
                Text(directory).foregroundStyle(K.C.faint)
                Text(filename).foregroundStyle(K.C.text)
            }
            .font(K.F.codeSmall.weight(.medium))
            .lineLimit(1).truncationMode(.head)

            switch repo {
            case .some(.some(let root)):
                Pill(text: (root as NSString).lastPathComponent, tone: .accent)
            case .some(.none):
                // The one that matters. Nothing is keeping a copy of this file, so there is no
                // diff to read later and no rewind that will bring it back.
                Pill(text: "NOT IN GIT", tone: .warn)
            case nil:
                EmptyView()
            }

            Spacer(minLength: K.S.sm)
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .background(hovering ? K.C.hover : K.C.raised, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .asButton { Self.reveal(path) }
        .hint("\(path) — outside this repository. Click to reveal it in the Finder.")
        .task(id: path) { repo = .some(Self.gitRoot(of: path)) }
    }

    private var directory: String {
        let parent = (path as NSString).deletingLastPathComponent
        return parent.isEmpty ? "" : parent + "/"
    }

    private var filename: String { (path as NSString).lastPathComponent }

    /// The Finder, from the app rather than through `model.reveal`.
    ///
    /// `/api/fs/reveal` resolves through `tree::resolve`, which confines every path to the
    /// repository and the Claude config — the traversal guard on a loopback API that any page in
    /// a browser can reach. A file in `/tmp` is exactly what it is there to refuse, so the request
    /// 400s and `reveal` drops it on the floor: the row looked dead and said nothing. Weakening
    /// the guard to light up one row would be the wrong trade by a distance.
    ///
    /// It does not need the daemon anyway. The path is already absolute, and the app is the host —
    /// the same reason `NSOpenPanel` replaced `/api/browse`. `model.reveal` keeps the round trip
    /// because its callers pass checkout-relative paths and only the daemon knows which checkout a
    /// lane is standing in.
    ///
    /// A scratch file the agent wrote and deleted again selects nothing at all, so that case falls
    /// back to opening the folder it was in.
    static func reveal(_ path: String) {
        let url = URL(fileURLWithPath: path)
        if FileManager.default.fileExists(atPath: path) {
            NSWorkspace.shared.activateFileViewerSelecting([url])
        } else {
            NSWorkspace.shared.open(url.deletingLastPathComponent())
        }
    }

    /// What is version-controlling this file, if anything: the nearest `.git` on the way up.
    ///
    /// `FileManager` rather than `git rev-parse`, because this answers one question about one
    /// path and a row in a list must not wait on a subprocess. It is a dozen `stat`s at the
    /// deepest, and it runs once per row rather than once per render.
    ///
    /// A submodule's `.git` is a file rather than a directory, which is why this asks whether the
    /// name exists rather than whether a directory does.
    static func gitRoot(of path: String) -> String? {
        var dir = (path as NSString).deletingLastPathComponent
        while dir.count > 1 {
            if FileManager.default.fileExists(atPath: (dir as NSString).appendingPathComponent(".git")) {
                return dir
            }
            dir = (dir as NSString).deletingLastPathComponent
        }
        return nil
    }
}
