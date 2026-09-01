import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// The conversation, and the box you type in.
///
/// A rail beside the stage rather than the thing itself: the turn is the unit of work, and the
/// chat is how you steer it. Tool calls appear here as one dim line each, in the order they
/// happened, so the shape of what the agent is doing is visible without competing with what it
/// changed — and each one opens onto exactly what was passed to it.
struct ChatRail: View {
    @Bindable var model: SessionModel
    @FocusState private var composerFocused: Bool
    /// A drag is over the conversation. The whole column takes the drop — a target you have to aim
    /// at is a target you miss, and the composer is the smallest thing on screen.
    @State private var dropping = false

    /// Whether the view is following the stream. Scrolling up to read releases it — a pane that
    /// drags you back to the bottom mid-sentence is worse than one that never followed.
    @State private var pinned = true
    @State private var lastFollow = Date.distantPast
    /// The scroll the throttle turned away, waiting out its window. At most one.
    @State private var trailing: Task<Void, Never>?

    /// How many turns are drawn before the conversation offers the rest.
    ///
    /// This is what makes the pane land where it is told. A lazy stack knows the height of the
    /// rows it has built and *estimates* the rest, and a turn's card is anything from two lines
    /// to a megabyte of tool output — so a jump to the end of three hundred of them is computed
    /// from three hundred guesses and lands in a region with nothing built in it. That is the
    /// white pane on load, and no amount of re-scrolling fixes an estimate. Ten rows is a bounded
    /// error, and the ones you cannot see are one click away rather than one scroll.
    private static let shown = 10
    @State private var showingAll = false
    /// A rescue is already running. Without it the geometry fires again on every scroll the
    /// rescue itself performs.
    @State private var rescuing = false

    var body: some View {
        VStack(spacing: 0) {
            transcript
            // A question holds the turn, so it sits where you are about to type — not in a
            // stage that may have switched to the preview while you were reading.
            //
            // Above the working bar, not below it: the question is the thing to do and the bar is
            // the thing waiting for it. They also both used to say "waiting for you" in amber,
            // stacked, which is one banner too many for one fact.
            if !model.pending.isEmpty {
                VStack(spacing: K.S.sm) {
                    ForEach(model.pending) { p in
                        if p.isQuestion {
                            QuestionCard(pending: p, model: model)
                        } else if p.isPlan {
                            PlanCard(pending: p, model: model)
                        } else {
                            ApprovalCard(pending: p, model: model)
                        }
                    }
                }
                .padding(.horizontal, K.S.xxl)
                .frame(maxWidth: 800, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.top, K.S.md)
                .transition(.asymmetric(
                    insertion: .move(edge: .bottom).combined(with: .opacity),
                    removal: .opacity))
            }
            // Proof it is alive, directly above the box you type into.
            //
            // It used to live at the top of the stage column, which is the wrong side of the
            // window: reading the conversation, the only evidence of a running turn was in a
            // pane you might not be looking at — and on a window too narrow for the stage there
            // was no working bar at all. A turn that is thinking for ninety seconds with nothing
            // on screen beside the composer reads as a turn that did not start.
            // A conversation running somewhere else, mirrored here as it is written.
            //
            // Worth saying out loud rather than leaving the turns to appear on their own: what is
            // on screen is not this window's work, and typing takes it over rather than joining
            // in. Two processes driving one `--resume` is a claim problem, and `AppState::claim`
            // is about lanes and working trees rather than conversations.
            if model.following {
                HStack(spacing: K.S.xs) {
                    Image(systemName: "dot.radiowaves.left.and.right")
                        .font(K.F.tiny)
                    Text("Following this session — it is running outside Keel. Sending takes it over.")
                        .font(K.F.micro)
                    Spacer()
                }
                .foregroundStyle(K.C.dim)
                .padding(.horizontal, K.S.sm).padding(.vertical, K.S.xs)
                .background(K.C.surface, in: RoundedRectangle(cornerRadius: K.R.sm))
                .padding(.horizontal, K.S.xxl)
                .frame(maxWidth: 800, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.top, K.S.sm)
                .transition(.opacity)
            }
            if model.running {
                WorkingBar(model: model)
                    .padding(.horizontal, K.S.xxl)
                    .frame(maxWidth: 800, alignment: .leading)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.top, K.S.sm)
                    .transition(.opacity)
            }
            Composer(model: model, focused: $composerFocused, dropping: $dropping)
        }
        .animation(K.M.enter, value: model.pending.count)
        .animation(K.M.settle, value: model.running)
        .animation(K.M.settle, value: model.following)
        .background(K.C.bg)
        .onDrop(of: [.fileURL, .image], isTargeted: $dropping) { model.take(drop: $0) }
        // The shortcuts themselves are handled by `WindowEvents`, which is never unmounted.
        // Focus is the one thing only this view can do, so it watches a counter.
        .onChange(of: model.focusComposerTick) { composerFocused = true }
    }

    private var transcript: some View {
        ScrollViewReader { proxy in transcript(proxy) }
    }

    private func transcript(_ proxy: ScrollViewProxy) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: K.S.xl) {
                // A pane with nothing in it says which of the reasons it is. It used to
                // draw the "ask for a change" hint over a session that was still being read,
                // and nothing at all if the read had left the lane empty.
                if model.turns.isEmpty, !model.replaying { hint }
                let hidden = showingAll ? 0 : max(0, model.turns.count - Self.shown)
                if hidden > 0 {
                    // Unpinned first. Asking for the earlier turns is asking to *read* them, and
                    // the content-height loop below would otherwise read the arrival of ten more
                    // cards as a reason to put you back at the end of the conversation.
                    ShowMore(count: hidden, up: true) { pinned = false; showingAll = true }
                }
                ForEach(Array(model.turns.enumerated()).dropFirst(hidden), id: \.element.id) { i, turn in
                    ChatTurn(turn: turn, number: i + 1, model: model)
                        .id("chat-\(turn.id)")
                }
            }
            .padding(.horizontal, K.S.xxl)
            .padding(.vertical, K.S.xl)
            .frame(maxWidth: 800, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .center)
        }
        .scrollBounceBehavior(.basedOnSize)
        // The tail token is watched by a view of its own, not from here.
        //
        // Reading it in this `body` registered the dependency against the whole pane, so every
        // text delta invalidated the transcript — the `ForEach` over every turn, each `ChatTurn`,
        // and the composer with it. The 80 ms coalescing throttled the *scroll*; nothing throttled
        // the rebuild. `TailFollower` draws nothing and is the only thing that re-evaluates.
        .overlay { TailFollower(model: model) { follow(proxy) } }
        // A task, not an onChange: opening a session bumps `pinTick` before this pane exists,
        // so the change had no listener and the transcript opened at the top. A task with that
        // id runs on appear as well, after the rows have laid out.
        .task(id: model.pinTick) {
            pinned = true
            showingAll = false
            // The seed. Landing short is expected — the rows below have never been built, so
            // this is a scroll into an estimate — and the content-height loop below is what
            // corrects it as they are.
            toBottom(proxy)
        }
        .followsTail($pinned)
        // The one that actually keeps the pane at the end, and the answer to every version of
        // "it went white" in this file's history.
        //
        // A `LazyVStack` does not know how tall it is. It measures the rows it has built and
        // *estimates* the rest from them, so the content height is a guess that is revised every
        // time another row is created — and here the rows range from a two-line answer to a turn
        // with a hundred steps in it, so the guess is wrong by whole screens. A scroll to the end
        // is a position in that guess. When the guess is then corrected downwards, the offset
        // that was the end is now past the end, and past the end of a scroll view is the window's
        // own background: white, with the conversation above it, exactly as reported.
        //
        // Nothing was watching for that. `TailFollower` fires on the *model* changing, and an
        // estimate being corrected is pure layout — no delta, no token, no event. So the pane
        // scrolled to a wrong number once and then sat in it.
        //
        // Every change in content height re-asserts the end, which turns a one-shot guess into a
        // loop that converges: each correction builds more rows, each rebuild is a better
        // estimate, and the scroll lands truer until the height stops moving. It cannot yank
        // anybody: `pinned` is false the moment a person scrolls up, and that is the only thing
        // this reads.
        .onScrollGeometryChange(for: CGFloat.self) { $0.contentSize.height } action: { was, now in
            guard pinned, now != was else { return }
            follow(proxy)
        }
        // The rescue: a transcript scrolled clean off its own content.
        //
        // This is the blank pane people report — white, with the conversation reachable only by
        // scrolling up out of it by hand and back down. It has more than one cause and all of
        // them land here, so the check is on the geometry rather than on any one of them:
        // `scrollPosition(id:)` holds a *row*, and a row id stops resolving when the turns are
        // replaced (a replay builds new `Turn`s, so every id is new) or removed (the fresh-session
        // retry drops the turn it failed on, and `/clear` drops all of them). An anchor that
        // names nothing leaves the offset exactly where it was, past the end of content that
        // shrank underneath it — and nothing in SwiftUI brings it back.
        //
        // Measured as *how much of the conversation is on screen*, which is the thing being
        // complained about. The first version of this asked whether the offset was past the end
        // of the content entirely, and that is only the worst case: an anchor that stops
        // resolving leaves the offset wherever it was, so the pane settles anywhere past its
        // legal maximum — usually with a sliver of the last turn at the top and the rest of the
        // window white. Scrolling by hand cannot produce that state at all, because it clamps.
        .onScrollGeometryChange(for: Bool.self) { g in
            Self.blank(offset: g.contentOffset.y, content: g.contentSize.height,
                       viewport: g.containerSize.height)
        } action: { _, blank in
            guard blank else { rescuing = false; return }
            rescue(proxy)
        }
        // The end of a turn is the one update the coalescing window can swallow whole: the
        // last delta arrives and there is no next one to correct the short scroll.
        .onChange(of: model.running) {
            guard pinned else { return }
            toBottom(proxy)
        }
        // In the middle of the pane, not as the first row of it: a line of small grey text
        // at the top left of an empty transcript is a wait nobody sees, and the read it is
        // reporting is the slowest one in the app.
        .overlay {
            if model.replaying, model.turns.isEmpty { openingNote }
        }
        .overlay(alignment: .bottom) {
            if !pinned && model.running {
                JumpToLatest {
                    pinned = true
                    withAnimation(K.M.settle) { toBottom(proxy) }
                }
                .transition(.opacity)
            }
        }
    }

    /// Coalesced. A reply arrives as many small deltas, and scrolling on each of them competes
    /// with the wheel and makes the pane feel like it is resisting.
    ///
    /// Coalesced on *both* edges, which is the half that was missing. A leading-edge throttle
    /// drops the last delta of every burst, and a burst ends every time the agent stops talking
    /// to go and run something — so the pane sat a paragraph short of the end for as long as the
    /// tool took, and the next thing written started off-screen. That is what "it stopped
    /// autoscrolling" is: not following that switched off, following that is permanently one
    /// window behind. `onChange(of: model.running)` already patched exactly one of those pauses,
    /// the very last one, which is why the end of a turn was the only part that reliably landed.
    private func follow(_ proxy: ScrollViewProxy) {
        guard pinned else { return }
        let now = Date()
        guard now.timeIntervalSince(lastFollow) > 0.08 else {
            // Replaced rather than stacked: within one window every delta wants the same thing,
            // and the last one to ask is the one holding the newest tail.
            trailing?.cancel()
            trailing = Task {
                try? await Task.sleep(for: .milliseconds(80))
                guard !Task.isCancelled, pinned else { return }
                lastFollow = Date()
                toBottom(proxy)
            }
            return
        }
        lastFollow = now
        toBottom(proxy)
    }

    /// To the end of the conversation: the last turn, held at the bottom of the pane.
    ///
    /// Held against a *row*, never against an offset: an offset into a lazy stack is a number
    /// computed from the estimated heights of rows it has never built, and here those rows range
    /// from a two-line answer to a turn with a hundred steps in it.
    ///
    /// Asked for as an *action* rather than held as state. `scrollPosition(id:)` keeps the row id
    /// in a binding, and a binding already holding the value you assign is a no-op — so through a
    /// streaming turn, whose last row's id never changes, every scroll after the first asked for
    /// something that was already true and the pane quietly stopped following. A `scrollTo`
    /// re-resolves the row every time it is called and a row that is gone is a no-op rather than
    /// a position nothing can leave. The Trace has always followed this way, and the Trace is not
    /// the pane people report.
    ///
    /// Deliberately not animated. This is a tail following a live stream, like a terminal, and an
    /// easing curve restarted twelve times a second is what "flaky" looks like — the explicit
    /// jumps animate, because those are deliberate moves.
    private func toBottom(_ proxy: ScrollViewProxy) {
        guard let last = model.turns.last else { return }
        proxy.scrollTo("chat-\(last.id)", anchor: .bottom)
    }

    /// Less than a quarter of the pane has any conversation in it. The decision on its own, so
    /// it can be asserted without a scroll view.
    ///
    /// At rest at the end, a full viewport of content is showing, so this is a long way from
    /// firing. Only an overscroll can approach it — a fling would have to rubber-band three
    /// quarters of the window past the end — and even then the rescue puts the pane exactly where
    /// the band was going to settle anyway. Content shorter than the viewport is never blank: it
    /// cannot scroll, so it is on screen by construction.
    static func blank(offset: CGFloat, content: CGFloat, viewport: CGFloat) -> Bool {
        content > viewport && content - offset < viewport / 4
    }

    /// Put a pane that is showing nothing back at the end of the conversation — and say so.
    ///
    /// It retries, which is the half that was missing and the reason people kept reporting a
    /// white pane after this existed. `onScrollGeometryChange` fires on a *transition*, so one
    /// attempt was all there ever was: a scroll issued while the rows it has to measure are
    /// still being built lands short, the value is already `true`, nothing fires again, and the
    /// pane stays white until the person scrolls out of it by hand. Which is exactly the report.
    ///
    /// The last resort is folding the conversation back to its most recent turns. Content the
    /// stack has actually measured is content a scroll cannot miss, and a person who expanded
    /// the history is better off at the end of the conversation than in a white rectangle.
    ///
    /// Reported, because this is exactly the failure the bar names — a pane showing a thing it
    /// cannot explain — and the only reason it lasted this long is that it is silent. It crashes
    /// nothing and fails no request; a person scrolls out of it and carries on, and we never hear.
    /// Counts and flags only, never a prompt or a path.
    private func rescue(_ proxy: ScrollViewProxy) {
        guard !rescuing, model.turns.last != nil else { return }
        rescuing = true
        Telemetry.warn("transcript scrolled off its own content", [
            "turns": "\(model.turns.count)",
            "replaying": "\(model.replaying)",
            "running": "\(model.running)",
            "expanded": "\(showingAll)",
        ])
        Task { @MainActor in
            pinned = true
            for attempt in 0..<3 {
                if attempt == 2 { showingAll = false }
                toBottom(proxy)
                try? await Task.sleep(for: .milliseconds(120))
            }
            // Cleared here as well as by the geometry: a pane that is somehow still blank must
            // be able to ask again the next time anything moves.
            rescuing = false
        }
    }

    /// What an empty lane is for.
    ///
    /// A second agent is only worth starting if it has its own job, so this says what the good
    /// jobs are — and, when another lane is already editing, warns that they share one working
    /// tree. A lane can have a checkout of its own; this one chose not to, so the honest thing is
    /// to say so at the point where it matters rather than let two agents fight over the files.
    /// While a transcript is being read. Slow on purpose — hundreds of messages off disk — and
    /// the one wait in this pane long enough to look like a failure.
    private var openingNote: some View {
        VStack(spacing: K.S.md) {
            ProgressView()
            Text("Opening this session…").font(K.F.body).foregroundStyle(K.C.dim)
        }
        .transition(.opacity)
    }

    private var hint: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            if model.isolated {
                Text("This feature gets its own checkout and branch on the first send, so it can "
                     + "edit while the others do. Finish it from the rail to merge.")
                    .font(K.F.micro).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let lanes = model.lanes, lanes.shown.count > 1 {
                VStack(alignment: .leading, spacing: K.S.xs) {
                    Text("A second agent, on the same files")
                        .font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                    Text(lanes.wouldOverlap(with: model)
                         ? "Another feature is editing right now. These share one working tree, so "
                           + "give this one reading or planning work — two agents writing the same "
                           + "files will overwrite each other."
                         : "These share one working tree. Good alongside work: reading, planning, "
                           + "reviewing what another feature just did, or resuming an old one.")
                        .font(K.F.micro).foregroundStyle(lanes.wouldOverlap(with: model) ? K.C.warn : K.C.dim)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(K.S.sm)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    (lanes.wouldOverlap(with: model) ? K.C.warn.wash : K.C.surface),
                    in: RoundedRectangle(cornerRadius: K.R.sm)
                )
            }

            VStack(alignment: .leading, spacing: K.S.xs) {
                Text("Ask for a change")
                    .font(K.F.small.weight(.semibold)).foregroundStyle(K.C.dim)
                ForEach(suggestions, id: \.self) { s in
                    Button { model.prompt = s } label: {
                        HStack(spacing: K.S.xs) {
                            Image(systemName: "arrow.turn.down.right")
                                .font(K.F.tiny).foregroundStyle(K.C.faint)
                            Text(s).font(K.F.small).foregroundStyle(K.C.faint)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .frame(maxWidth: 520, alignment: .leading)
    }

    /// Reading and planning first when another lane is writing, because those are the jobs that
    /// cannot collide.
    private var suggestions: [String] {
        if model.lanes?.wouldOverlap(with: model) == true {
            return ["Explain how this codebase is structured",
                    "Review the uncommitted changes and flag anything unfinished",
                    "Plan how to add a feature, without editing anything"]
        }
        return ["Explain how this codebase is structured",
                "Summarise the uncommitted changes",
                Flags.readiness ? "Fix the blocking readiness findings"
                                : "Plan how to add a feature, without editing anything"]
    }
}

/// One exchange, as a conversation.
///
/// Deliberately *not* the tool calls or the file counts: the stage beside this lists both, and two
/// panes printing the same numbers is the duplication that made the window hard to read. What
/// belongs here is what you asked, what it thought, and what it said back — plus a marker that
/// jumps the stage to the matching turn, which is cheaper than repeating its contents.
private struct ChatTurn: View {
    let turn: Turn
    let number: Int
    let model: SessionModel
    @State private var hovering = false
    @State private var copied: String?
    @State private var expanded = false

    /// How much of a pasted prompt is shown before it needs asking for. Lines for the display,
    /// characters for whether to offer the button at all — a `lineLimit` cannot say whether it
    /// truncated anything.
    static let promptLines = 14
    static let promptChars = 800

    /// The prompt as it is actually drawn. The comment below was right that `fixedSize` measures
    /// every line whether or not it is on screen, and wrong that `lineLimit` stops it — see
    /// `String.capped`.
    private var shown: String {
        expanded ? turn.prompt : turn.prompt.capped(Self.promptChars)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.half) {
            // Yours, on the right, in blue. Nothing above it: a message does not need a title.
            //
            // Except when nobody typed it. A finished background job is delivered as a turn, and
            // drawn as a blue bubble on the right it reads as something the person said — the
            // same misattribution that makes a resumed transcript unreadable when Claude Code's
            // own task notifications are rendered as chat.
            if turn.prompt.hasPrefix(SessionModel.jobPrefix) {
                JobReport(text: turn.prompt)
            } else {
                HStack(alignment: .bottom) {
                    Spacer(minLength: 64)
                    VStack(alignment: .trailing, spacing: K.S.xs) {
                        // Capped until asked. A prompt is often a paste — the largest in this
                        // repository's own history is 80 KB — and `fixedSize` measures every line
                        // of it whether or not it is on screen. Ninety-two turns took 264 ms to
                        // lay out, 16 ms once the pastes stopped being measured whole: opening a
                        // long session was a visible stall, and every keystroke in the composer
                        // paid it again.
                        Text(shown)
                            .font(K.F.body)
                            .foregroundStyle(K.C.text)
                            .textSelection(.enabled)
                            .lineLimit(expanded ? nil : Self.promptLines)
                            .fixedSize(horizontal: false, vertical: true)
                        if turn.prompt.count > Self.promptChars {
                            Button(expanded ? "Show less" : "Show all")
                                { withAnimation(K.M.quick) { expanded.toggle() } }
                                .buttonStyle(.plain)
                                .font(K.F.micro)
                                .foregroundStyle(K.C.accent)
                        }
                    }
                    .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
                    .background(K.C.accent.wash, in: RoundedRectangle(cornerRadius: K.R.md))
                    .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.accent.opacity(0.25), lineWidth: 1))
                    .frame(maxWidth: 560, alignment: .trailing)
                }
            }

            // What it did, in the order it did it.
            //
            // One merged blob of prose and a list of tool calls in another pane is not what the
            // agent did: it thought, said something, ran a command, read what came back, and said
            // something else. That sequence is the thing being reviewed, and reconstructing it
            // from two panes was left to the reader.
            if !turn.steps.isEmpty {
                VStack(alignment: .leading, spacing: K.S.sm) {
                    HStack(spacing: K.S.xs) {
                        Image(systemName: "sailboat.fill")
                            .font(K.F.tiny.weight(.semibold))
                        Text("Keel").font(K.F.micro.weight(.semibold))
                    }
                    .foregroundStyle(K.C.dim)

                    ForEach(turn.steps) { step in
                        switch step {
                        case .say(let block):
                            // Already parsed, on the model. Nothing here parses anything.
                            Markdown(blocks: block.blocks)
                                .padding(.trailing, K.S.lg)
                        case .think(let block):
                            ThinkingBlock(block: block)
                        case .call(let id):
                            if let call = turn.call(id) {
                                CallRow(call: call)
                            }
                        }
                    }
                }
                .padding(.top, K.S.sm)
            }

            // The caption Messages puts under a bubble — here, the turn and the ways to copy
            // it. Faint, and only under the pointer.
            HStack(spacing: K.S.sm) {
                Button {
                    model.focusedTurn = turn.id
                } label: {
                    HStack(spacing: K.S.tight) {
                        Text("Turn \(number)").font(K.F.micro)
                        Image(systemName: "arrow.right").font(K.F.ui(7, .bold))
                    }
                    .foregroundStyle(K.C.faint)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .hint("Show this turn in the trace")
                if !turn.text.isEmpty {
                    CopyChip(label: "reply", copied: copied == "reply") { put(turn.text, "reply") }
                        .opacity(hovering || copied != nil ? 1 : 0.4)
                }
                CopyChip(label: "both", copied: copied == "both") { put("> \(turn.prompt)\n\n\(turn.text)", "both") }
                    .opacity(hovering || copied != nil ? 1 : 0.4)
                Spacer()
            }
            .padding(.leading, K.S.xs)
            .frame(minHeight: 18)
            .contentShape(Rectangle())
            .animation(K.M.quick, value: hovering)
        }
        // The whole column is the hover target, gaps included: without a shape, the pointer
        // leaving a bubble for the transparent space beside the caption ended the hover, and
        // the chips faded out under the click.
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        // A second route, because a hover target is no use from the keyboard or a trackpad tap.
        .contextMenu {
            Button("Copy reply") { put(turn.text, "reply") }
                .disabled(turn.text.isEmpty)
            Button("Copy question and reply") { put("> \(turn.prompt)\n\n\(turn.text)", "both") }
            Divider()
            Button("Show in trace") { model.focusedTurn = turn.id }
        }
    }

    /// Copies the Markdown source rather than the rendered text: it is what you paste back into a
    /// message, an issue or a commit, and the rendering is only for reading here.
    private func put(_ text: String, _ which: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        copied = which
        Task { try? await Task.sleep(for: .seconds(1.4)); copied = nil }
    }
}

/// The copy affordance used wherever something is worth taking out of Keel: a reply, a console,
/// a command's output. Faint until it has something to say, and it says "copied" rather than
/// flashing — a button that gives no feedback is one people press twice.
struct CopyChip: View {
    let label: String
    let copied: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: K.S.tight) {
                Image(systemName: copied ? "checkmark" : "doc.on.doc")
                    .font(K.F.tiny.weight(.bold))
                Text(copied ? "copied" : label).font(K.F.micro)
            }
            .foregroundStyle(copied ? K.C.add : K.C.faint)
            .padding(.horizontal, K.S.snug).padding(.vertical, K.S.xxs)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Composer

struct Composer: View {
    @Bindable var model: SessionModel
    @FocusState.Binding var focused: Bool
    /// Owned by the rail, which is the thing you drop on.
    @Binding var dropping: Bool
    /// The completion being typed: which sigil started it, the query after it, and where it sits.
    ///
    /// One mechanism for `@` and `/` rather than two copies of it. They differ in three things —
    /// the character, where it is allowed, and what a pick does — and everything else about
    /// finding, ranking, showing and keying through them is identical.
    ///
    /// Derived from the prompt, not stored beside it. As `@State` set in `onChange` it was empty
    /// for any prompt that did not arrive one keystroke at a time — a restored draft, a palette
    /// insertion, a test — which is the same way the approval card managed to render invisible.
    /// State that mirrors other state is state that can be wrong.
    private var completing: Completion? { completion(in: model.prompt) }
    /// Which row is highlighted. This one is genuinely the view's own.
    @State private var pick = 0
    /// Dismissed with Escape, until the text changes again.
    @State private var dismissed = ""

    struct Completion: Equatable {
        enum Kind { case file, command }
        var kind: Kind
        var query: String
        var start: String.Index
    }

    /// What is being typed, if anything.
    ///
    /// Read from the prompt rather than tracked as the person types: a paste, an undo and a
    /// deletion all have to reach the same answer, and only the text knows.
    private func completion(in prompt: String) -> Completion? {
        // `/` only at the very start. Mid-sentence it is a path or a date, not a command, and a
        // picker that opens on `and/or` is a picker people learn to fight.
        if prompt.hasPrefix("/") {
            let query = String(prompt.dropFirst())
            if !query.contains(" "), !query.contains("\n") {
                return Completion(kind: .command, query: query, start: prompt.startIndex)
            }
        }
        guard let at = prompt.lastIndex(of: "@") else { return nil }
        // Only at a word start, so an email address or a `@path` already inserted is left alone.
        let before = at == prompt.startIndex ? " " : String(prompt[prompt.index(before: at)])
        guard before == " " || before == "\n" else { return nil }
        let query = String(prompt[prompt.index(after: at)...])
        guard !query.contains(" "), !query.contains("\n") else { return nil }
        return Completion(kind: .file, query: query, start: at)
    }

    /// One row: what it is called, and what it is.
    struct Hit: Equatable {
        var value: String
        var detail: String
    }

    /// At most eight, because a list that fills the window is a palette and there is one of those.
    private var hits: [Hit] {
        guard let c = completing, model.prompt != dismissed else { return [] }
        let all: [Hit]
        switch c.kind {
        case .file:
            all = model.files.map { Hit(value: $0, detail: "") }
        case .command:
            let own = SessionModel.ownCommands.map { Hit(value: $0.name, detail: $0.detail) }
            // Keel's own first: they are the two that do something to the task rather than ask
            // the agent for something, and there are two of them against eighty-odd.
            all = own + model.slashCommands
                .filter { name in !own.contains { $0.value == name } }
                .map { Hit(value: $0, detail: "") }
        }
        guard !c.query.isEmpty else { return Array(all.prefix(8)) }
        return all
            .compactMap { h in Fuzzy.score(c.query, in: h.value).map { (h, $0) } }
            .sorted { $0.1 > $1.1 }
            .prefix(8)
            .map(\.0)
    }

    /// Take the pick. A file becomes an attachment and the `@query` goes — it was the gesture, not
    /// text the agent should read. A command *is* the text, so it stays and waits for Return.
    private func take(_ hit: Hit) {
        guard let c = completing else { return }
        switch c.kind {
        case .file:
            model.prompt.removeSubrange(c.start...)
            model.mention(hit.value)
        case .command:
            model.prompt = "/" + hit.value
        }
        pick = 0
        focused = true
    }

    var body: some View {
        VStack(spacing: K.S.sm) {
            PinList(model: model)
            AttachmentStrip(model: model)
            completionList

            if !model.notes.isEmpty {
                Button {
                    model.prompt = model.commentsPrompt()
                    model.notes.removeAll()
                    focused = true
                } label: {
                    HStack(spacing: K.S.snug) {
                        Image(systemName: "text.bubble.fill").font(K.F.tiny)
                        Text("Send \(model.notes.count) review comment\(model.notes.count == 1 ? "" : "s")")
                    }
                }
                .buttonStyle(QuietButton(tone: K.C.accent))
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            if !model.queued.isEmpty {
                HStack(spacing: K.S.snug) {
                    Image(systemName: "arrow.down.circle").font(K.F.tiny)
                    Text("\(model.queued.count) queued — they run in order when this turn ends")
                        .font(K.F.micro)
                }
                .foregroundStyle(K.C.faint)
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            if let err = model.lastError {
                ErrorRow(message: err, fix: model.lastFix) {
                    model.lastError = nil; model.lastFix = nil
                }
            }

            VStack(spacing: 0) {
                field
                controls.overlay(alignment: .top) { Hairline() }
            }
            .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.lg))
            .overlay(
                RoundedRectangle(cornerRadius: K.R.lg)
                    .stroke(dropping ? K.C.accent : (focused ? K.C.lineStrong : K.C.line),
                            lineWidth: dropping ? 2 : 1)
            )
            .animation(K.M.quick, value: focused)
            .animation(K.M.quick, value: dropping)
            memoryNote
        }
        .padding(.horizontal, K.S.xxl)
        .padding(.top, K.S.sm)
        .padding(.bottom, K.S.lg)
        .frame(maxWidth: 800, alignment: .leading)
        .frame(maxWidth: .infinity, alignment: .center)
        .background(K.C.bg)
    }

    /// The `@` file picker the paperclip's tooltip and `docs/features.md` both promised, and the
    /// `/` command picker that closes the gap with the terminal.
    ///
    /// Claude Code reports every command it accepts on its `init` record — built-ins, the
    /// project's own, every plugin's and every skill — so this is *its* list rather than one Keel
    /// assembled and would then have to keep in step.
    @ViewBuilder
    private var completionList: some View {
        if !hits.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(hits.enumerated()), id: \.element.value) { i, hit in
                    HoverRow(selected: i == pick) {
                        HStack(spacing: K.S.sm) {
                            Image(systemName: completing?.kind == .command
                                  ? "chevron.right.square" : "doc")
                                .font(K.F.tiny).foregroundStyle(K.C.faint)
                                .accessibilityHidden(true)
                            Text(Fuzzy.highlight(completing?.query ?? "", in: hit.value))
                                .font(K.F.small).foregroundStyle(K.C.text)
                                .lineLimit(1)
                                .truncationMode(completing?.kind == .command ? .tail : .head)
                            if !hit.detail.isEmpty {
                                Text(hit.detail).font(K.F.tiny).foregroundStyle(K.C.dim)
                                    .lineLimit(1)
                            }
                            Spacer(minLength: 0)
                        }
                        .padding(.vertical, K.S.hair)
                    } action: {
                        take(hit)
                    }
                }
            }
            .padding(.vertical, K.S.xs)
            .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.md))
            .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var field: some View {
        TextField("Ask Keel to change, explain, or review…", text: $model.prompt, axis: .vertical)
            .textFieldStyle(.plain)
            .font(K.F.reading)
            .lineSpacing(3)
            .lineLimit(1...8)
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
            .focused($focused)
            // The button has always drawn a `return` glyph; until now Return only inserted a
            // newline and ⌘Return was the real key, so the control lied about itself. ⇧Return
            // still makes a new line, which is the convention every chat composer uses.
            .onSubmit { if hits.isEmpty { model.send() } }
            .onPasteCommand(of: [.png, .tiff, .fileURL, .plainText]) { _ in
                if !model.takePaste(.general) {
                    model.prompt += NSPasteboard.general.string(forType: .string) ?? ""
                }
            }
            // Native vertical-axis layout measures wrapped lines, not only explicit newlines, so
            // the composer grows naturally until eight lines and then becomes scrollable.
            .onChange(of: model.prompt) {
                if model.prompt.count > SessionModel.longPaste { model.fileLongText() }
                pick = 0
            }
            // Arrow keys and Return belong to the list while it is up, and to the composer
            // otherwise — the same rule the palette follows.
            .onKeyPress(.upArrow) {
                guard !hits.isEmpty else { return .ignored }
                pick = max(0, pick - 1)
                return .handled
            }
            .onKeyPress(.downArrow) {
                guard !hits.isEmpty else { return .ignored }
                pick = min(hits.count - 1, pick + 1)
                return .handled
            }
            .onKeyPress(.tab) {
                guard !hits.isEmpty else { return .ignored }
                take(hits[pick])
                return .handled
            }
            .onKeyPress(.return) {
                guard !hits.isEmpty else { return .ignored }
                // A file is inserted and you carry on typing; a command *is* the whole prompt, so
                // completing it and sending it are one gesture, the way the terminal does it.
                let kind = completing?.kind
                take(hits[pick])
                if kind == .command { model.send() }
                return .handled
            }
            .onKeyPress(.escape) {
                guard !hits.isEmpty else { return .ignored }
                dismissed = model.prompt
                return .handled
            }
            // ⌃W deletes the word behind the cursor, the way every shell does. AppKit binds that
            // to ⌥⌫ and leaves ⌃W unbound, and this is a box people reach for straight out of a
            // terminal. Sent to the field editor rather than done to `model.prompt` here: the
            // caret is the text view's to know, and deleting the last word of the whole prompt is
            // the wrong edit whenever the caret is not at the end.
            .onKeyPress(.init("w"), phases: .down) { press in
                guard press.modifiers.contains(.control) else { return .ignored }
                let deleteWord = #selector(NSStandardKeyBindingResponding.deleteWordBackward(_:))
                return NSApp.sendAction(deleteWord, to: nil, from: nil) ? .handled : .ignored
            }
    }

    @ViewBuilder
    private var memoryNote: some View {
        if let note = model.remembered {
            HStack(spacing: K.S.xs) {
                Image(systemName: "brain").font(K.F.tiny).foregroundStyle(K.C.add)
                Text("Remembered in CLAUDE.md: \(note)").font(K.F.micro).foregroundStyle(K.C.dim)
                    .lineLimit(1)
                Spacer()
            }
        }
    }

    private var controls: some View {
        HStack(spacing: K.S.sm) {
            ModeToggle(mode: $model.mode)
            ModelPicker(model: model)
            Button { model.chooseAttachments() } label: {
                Image(systemName: "paperclip").font(K.F.micro)
                    .frame(width: 24, height: 22)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain).foregroundStyle(K.C.faint)
            .hint("Attach files (or paste, drop, or type @)")
            Spacer()
            // Stop belongs to the working bar directly above, and having it here too put two of
            // them forty pixels apart. What it cost was worse than the duplication: it *replaced*
            // Send while a turn ran, and `send()` while running queues — so the queue the composer
            // advertises ("N queued — they run in order when this turn ends") could not be added
            // to by mouse at all.
            Button {
                model.send()
            } label: {
                HStack(spacing: K.S.snug) {
                    Text(model.running ? "Queue" : "Send")
                    Image(systemName: "return").font(K.F.tiny.weight(.bold))
                }
            }
            .buttonStyle(FilledButton())
            .disabled(model.prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            .hint(model.running
                  ? "Queue this — it runs when the turn above finishes (Return, or ⌘Return)."
                  : "Send (Return, or ⌘Return). ⇧Return for a new line.")
        }
        .padding(.horizontal, K.S.md)
        .padding(.vertical, K.S.half)
    }
}

/// Plan or Auto. Named for what it does to the repository, not for a permission mode string.
struct ModeToggle: View {
    @Binding var mode: String

    var body: some View {
        HStack(spacing: 0) {
            segment("Plan", "plan", "explores and proposes, changes nothing")
            segment("Auto", "acceptEdits", "edits files, and asks before running commands")
        }
        .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
    }

    private func segment(_ label: String, _ value: String, _ help: String) -> some View {
        let on = mode == value
        return Text(label)
            .font(K.F.micro.weight(on ? .semibold : .regular))
            .foregroundStyle(on ? K.C.text : K.C.faint)
            .padding(.horizontal, K.S.sm).padding(.vertical, K.S.tight)
            .background(
                RoundedRectangle(cornerRadius: K.R.sm - 1)
                    .fill(on ? K.C.raised : .clear)
                    .padding(K.S.hair)
            )
            .contentShape(Rectangle())
            .asButton { mode = value }
            .hint("\(label) — \(help) (⇧⇥ switches)")
            .accessibilityAddTraits(on ? .isSelected : [])
    }
}




/// The model, the way `/model` picks it in the terminal. Default means the CLI's own choice.
struct ModelPicker: View {
    let model: SessionModel

    private var choices: [(value: String, label: String)] { model.provider.models }

    var body: some View {
        let _ = model.modelTick
        Menu {
            ForEach(choices, id: \.value) { value, label in
                Button {
                    model.claudeModel = value
                } label: {
                    if model.claudeModel == value {
                        Label(label, systemImage: "checkmark")
                    } else {
                        Text(label)
                    }
                }
            }
        } label: {
            Text(choices.first { $0.value == model.claudeModel }?.label ?? model.claudeModel)
                .font(K.F.micro).foregroundStyle(K.C.dim)
        }
        .menuStyle(.borderlessButton).fixedSize()
        .hint("Which model answers — the same choice `/model` makes in the terminal. Default "
              + "leaves it to the CLI's own config. Applies from the next turn.")
    }
}


/// A background job coming back, drawn as what it is: the machine reporting, not the person
/// asking. Collapsed to its first line, because the log underneath is usually the boring half and
/// the exit code is the part being read.
private struct JobReport: View {
    let text: String
    @State private var open = false

    private var headline: String { text.split(separator: "\n").first.map(String.init) ?? text }
    private var rest: String {
        text.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false)
            .dropFirst().first.map(String.init)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "binoculars.fill")
                    .font(K.F.micro).foregroundStyle(K.C.dim)
                    .accessibilityHidden(true)
                Text(headline).font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                Spacer(minLength: 0)
                if !rest.isEmpty {
                    Text(open ? "hide output" : "output")
                        .font(K.F.micro).foregroundStyle(K.C.faint)
                }
            }
            .contentShape(Rectangle())
            .asButton { withAnimation(K.M.quick) { open.toggle() } }

            if open, !rest.isEmpty {
                Text(rest)
                    .font(K.F.codeSmall).foregroundStyle(K.C.dim)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(K.S.sm)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))
    }
}
