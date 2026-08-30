import AppKit
import SwiftUI

/// The conversation, and the box you type in.
///
/// A rail beside the stage rather than the thing itself: the turn is the unit of work, and the
/// chat is how you steer it. Tool calls appear here as one dim line each so the shape of what the
/// agent is doing is visible without competing with what it changed.
struct ChatRail: View {
    @Bindable var model: SessionModel
    @FocusState private var composerFocused: Bool

    /// Whether the view is following the stream. Scrolling up to read releases it — a pane that
    /// drags you back to the bottom mid-sentence is worse than one that never followed.
    @State private var pinned = true
    @State private var lastFollow = Date.distantPast

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
            if model.running {
                WorkingBar(model: model)
                    .padding(.horizontal, K.S.xxl)
                    .frame(maxWidth: 800, alignment: .leading)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.top, K.S.sm)
                    .transition(.opacity)
            }
            Composer(model: model, focused: $composerFocused)
        }
        .animation(K.M.enter, value: model.pending.count)
        .animation(K.M.settle, value: model.running)
        .background(K.C.bg)
        // The shortcuts themselves are handled by `WindowEvents`, which is never unmounted.
        // Focus is the one thing only this view can do, so it watches a counter.
        .onChange(of: model.focusComposerTick) { composerFocused = true }
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: K.S.xl) {
                    if model.turns.isEmpty { hint }
                    ForEach(Array(model.turns.enumerated()), id: \.element.id) { i, turn in
                        ChatTurn(turn: turn, number: i + 1, model: model)
                            .id("chat-\(turn.id)")
                    }
                    // An anchor at the very end: scrolling to the last turn stops at its top when
                    // that turn is taller than the pane, which is exactly the case while a long
                    // reply is streaming.
                    Color.clear.frame(height: 1).id(Self.bottom)
                }
                .padding(.horizontal, K.S.xxl)
                .padding(.vertical, K.S.xl)
                .frame(maxWidth: 800, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .center)
            }
            .scrollBounceBehavior(.basedOnSize)
            // Watches the reply text as well as the tool calls. It only watched calls before, and
            // a reply arrives as text deltas — so the pane sat still through the entire answer.
            .onChange(of: tailToken) {
                guard pinned else { return }
                // Coalesced. A reply arrives as many small deltas, and calling `scrollTo` on each
                // of them competes with the wheel and makes the pane feel like it is resisting.
                let now = Date()
                guard now.timeIntervalSince(lastFollow) > 0.08 else { return }
                lastFollow = now
                proxy.scrollTo(Self.bottom, anchor: .bottom)
            }
            // A task, not an onChange: opening a session bumps `pinTick` before this pane
            // exists, so the change had no listener and the transcript opened at the top.
            // A task with that id runs on appear as well, after the rows have laid out.
            .task(id: model.pinTick) {
                pinned = true
                try? await Task.sleep(for: .milliseconds(60))
                proxy.scrollTo(Self.bottom, anchor: .bottom)
            }
            .onScrollGeometryChange(for: Bool.self) { geometry in
                // Within a line or two of the end counts as "at the end": demanding exactness
                // means one stray pixel silently turns following off.
                geometry.contentOffset.y + geometry.containerSize.height
                    >= geometry.contentSize.height - 24
            } action: { was, atBottom in
                // Only on a transition. Assigning on every scroll event republishes state for the
                // whole pane mid-gesture, which is its own source of stutter.
                if was != atBottom { pinned = atBottom }
            }
            .overlay(alignment: .bottom) {
                if !pinned && model.running {
                    JumpToLatest {
                        pinned = true
                        withAnimation(K.M.settle) { proxy.scrollTo(Self.bottom, anchor: .bottom) }
                    }
                    .transition(.opacity)
                }
            }
        }
    }

    private static let bottom = "chat-bottom"

    /// Everything that means "there is more text below", as one value. Includes the reply length,
    /// which is what actually grows while an answer streams.
    private var tailToken: String {
        let last = model.turns.last
        return "\(model.turns.count)-\(last?.text.count ?? 0)-\(last?.calls.count ?? 0)"
    }

    /// What an empty lane is for.
    ///
    /// A second agent is only worth starting if it has its own job, so this says what the good
    /// jobs are — and, when another lane is already editing, warns that they share one working
    /// tree. Keel has no worktree isolation on purpose; the honest thing is to say so at the point
    /// where it matters rather than let two agents fight over the same files.
    private var hint: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            if model.isolated {
                Text("This feature gets its own checkout and branch on the first send, so it can "
                     + "edit while the others do. Finish it from the rail to merge.")
                    .font(K.F.micro).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let lanes = model.lanes, lanes.lanes.count > 1 {
                VStack(alignment: .leading, spacing: K.S.xs) {
                    Text("A second agent, on the same files")
                        .font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                    Text(lanes.wouldOverlap
                         ? "Another feature is editing right now. These share one working tree, so "
                           + "give this one reading or planning work — two agents writing the same "
                           + "files will overwrite each other."
                         : "These share one working tree. Good alongside work: reading, planning, "
                           + "reviewing what another feature just did, or resuming an old one.")
                        .font(K.F.micro).foregroundStyle(lanes.wouldOverlap ? K.C.warn : K.C.dim)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(K.S.sm)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    (lanes.wouldOverlap ? K.C.warn.wash : K.C.surface),
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
        if model.lanes?.wouldOverlap == true {
            return ["Explain how this codebase is structured",
                    "Review the uncommitted changes and flag anything unfinished",
                    "Plan how to add a feature, without editing anything"]
        }
        return ["Explain how this codebase is structured",
                "Summarise the uncommitted changes",
                "Fix the blocking readiness findings"]
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

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.half) {
            // Yours, on the right, in blue. Nothing above it: a message does not need a title.
            HStack(alignment: .bottom) {
                Spacer(minLength: 64)
                Text(turn.prompt)
                    .font(K.F.body)
                    .foregroundStyle(K.C.text)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
                    .background(K.C.accent.wash, in: RoundedRectangle(cornerRadius: K.R.md))
                    .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.accent.opacity(0.25), lineWidth: 1))
                    .frame(maxWidth: 560, alignment: .trailing)
            }

            if !turn.thinking.isEmpty {
                DisclosureGroup {
                    Text(turn.thinking)
                        .font(K.F.codeSmall).italic()
                        .foregroundStyle(K.C.faint)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.leading, K.S.sm)
                        .overlay(alignment: .leading) {
                            Rectangle().fill(K.C.line).frame(width: 2)
                        }
                } label: {
                    Text("thinking").font(K.F.micro).foregroundStyle(K.C.faint)
                }
                .padding(.leading, K.S.xs)
            }

            // The reply, on the left, in grey. No border: the fill is the shape.
            // The reply, on the left, as prose: it is the long half, and a box around three
            // paragraphs and a table is a box around the page.
            if !turn.text.isEmpty {
                VStack(alignment: .leading, spacing: K.S.sm) {
                    HStack(spacing: K.S.xs) {
                        Image(systemName: "sailboat.fill")
                            .font(K.F.tiny.weight(.semibold))
                        Text("Keel").font(K.F.micro.weight(.semibold))
                    }
                    .foregroundStyle(K.C.dim)
                    Markdown(turn.text)
                        .padding(.trailing, K.S.lg)
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

private struct CopyChip: View {
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
    @State private var dropping = false
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
                ErrorRow(message: err) { model.lastError = nil }
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
            .onDrop(of: [.fileURL], isTargeted: $dropping) { providers in
                for p in providers {
                    _ = p.loadObject(ofClass: URL.self) { url, _ in
                        guard let url else { return }
                        Task { @MainActor in model.attach(fileURL: url) }
                    }
                }
                return true
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
