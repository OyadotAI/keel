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
            if !model.pending.isEmpty {
                Hairline()
                VStack(spacing: K.S.sm) {
                    ForEach(model.pending) { p in
                        if p.isQuestion {
                            QuestionCard(pending: p, model: model)
                        } else {
                            ApprovalCard(pending: p, model: model)
                        }
                    }
                }
                .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
                .background(K.C.accent.opacity(0.06))
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
            Hairline()
            Composer(model: model, focused: $composerFocused)
        }
        .animation(K.M.quick, value: model.pending.count)
        .background(K.C.surface)
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
                .padding(.horizontal, K.S.xl)
                .padding(.vertical, K.S.lg)
                .frame(maxWidth: 760, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
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
            .onChange(of: model.pinTick) {
                pinned = true
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
                    Button {
                        pinned = true
                        withAnimation(K.M.settle) { proxy.scrollTo(Self.bottom, anchor: .bottom) }
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
                Text("This lane gets its own checkout and branch on the first send, so it can "
                     + "edit while the others do. Finish it from the rail to merge.")
                    .font(K.F.micro).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let lanes = model.lanes, lanes.lanes.count > 1 {
                VStack(alignment: .leading, spacing: K.S.xs) {
                    Text("A second agent, on the same files")
                        .font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                    Text(lanes.wouldOverlap
                         ? "Another lane is editing right now. These share one working tree, so "
                           + "give this one reading or planning work — two agents writing the same "
                           + "files will overwrite each other."
                         : "These share one working tree. Good alongside work: reading, planning, "
                           + "reviewing what another lane just did, or resuming an old session.")
                        .font(K.F.micro).foregroundStyle(lanes.wouldOverlap ? K.C.warn : K.C.dim)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(K.S.sm)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    (lanes.wouldOverlap ? K.C.warn.opacity(0.08) : K.C.surface),
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
                                .font(.system(size: 10)).foregroundStyle(K.C.faint)
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
        VStack(alignment: .leading, spacing: 6) {
            // Yours, on the right, in blue. Nothing above it: a message does not need a title.
            HStack(alignment: .bottom) {
                Spacer(minLength: 64)
                Text(turn.prompt)
                    .font(K.F.body)
                    .foregroundStyle(K.C.text)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
                    .background(K.C.accent.opacity(0.10), in: RoundedRectangle(cornerRadius: K.R.md))
                    .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.accent.opacity(0.25), lineWidth: 1))
                    .frame(maxWidth: 560, alignment: .trailing)
            }

            if !turn.thinking.isEmpty {
                DisclosureGroup {
                    Text(turn.thinking)
                        .font(K.F.mono(11)).italic()
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
                Markdown(turn.text)
                    .padding(.trailing, K.S.lg)
            }

            // The caption Messages puts under a bubble — here, the turn and the ways to copy
            // it. Faint, and only under the pointer.
            HStack(spacing: K.S.sm) {
                Button {
                    model.focusedTurn = turn.id
                } label: {
                    HStack(spacing: 3) {
                        Text("Turn \(number)").font(K.F.micro)
                        Image(systemName: "arrow.right").font(.system(size: 7, weight: .bold))
                    }
                    .foregroundStyle(K.C.faint)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Show this turn in the trace")
                if !turn.text.isEmpty {
                    CopyChip(label: "reply", copied: copied == "reply") { put(turn.text, "reply") }
                }
                CopyChip(label: "both", copied: copied == "both") { put("> \(turn.prompt)\n\n\(turn.text)", "both") }
                Spacer()
            }
            .padding(.leading, K.S.xs)
            .frame(minHeight: 18)
            .contentShape(Rectangle())
            .opacity(hovering || copied != nil ? 1 : 0)
            .allowsHitTesting(hovering || copied != nil)
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
            HStack(spacing: 3) {
                Image(systemName: copied ? "checkmark" : "doc.on.doc")
                    .font(.system(size: 10, weight: .bold))
                Text(copied ? "copied" : label).font(K.F.micro)
            }
            .foregroundStyle(copied ? K.C.add : K.C.faint)
            .padding(.horizontal, 5).padding(.vertical, 2)
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

    /// Two lines, growing to eight. A composer that starts a third of the pane tall is a composer
    /// that has taken space from the conversation for nothing.
    private var height: CGFloat {
        let lines = model.prompt.reduce(1) { $1 == "\n" ? $0 + 1 : $0 }
        return min(max(CGFloat(lines) * 16 + 20, 52), 150)
    }

    var body: some View {
        VStack(spacing: K.S.sm) {
            PinList(model: model)
            AttachmentStrip(model: model)

            if !model.notes.isEmpty {
                Button {
                    model.prompt = model.commentsPrompt()
                    model.notes.removeAll()
                    focused = true
                } label: {
                    HStack(spacing: 5) {
                        Image(systemName: "text.bubble.fill").font(.system(size: 10))
                        Text("Send \(model.notes.count) review comment\(model.notes.count == 1 ? "" : "s")")
                    }
                }
                .buttonStyle(QuietButton(tone: K.C.accent))
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            if !model.queued.isEmpty {
                HStack(spacing: 5) {
                    Image(systemName: "arrow.down.circle").font(.system(size: 10))
                    Text("\(model.queued.count) queued — they run in order when this turn ends")
                        .font(K.F.micro)
                }
                .foregroundStyle(K.C.faint)
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            if let err = model.lastError {
                Text(err).font(K.F.small).foregroundStyle(K.C.del)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }

            field
            controls
        }
        .padding(.horizontal, K.S.xl)
        .padding(.vertical, K.S.md)
        .frame(maxWidth: 760, alignment: .leading)
    }

    private var field: some View {
        ZStack(alignment: .topLeading) {
            RoundedRectangle(cornerRadius: K.R.md)
                .fill(K.C.well)
                .overlay(
                    RoundedRectangle(cornerRadius: K.R.md)
                        .stroke(dropping ? K.C.accent : (focused ? K.C.lineStrong : K.C.line),
                                lineWidth: dropping ? 2 : 1)
                )

            if model.prompt.isEmpty {
                Text("Describe a change…   paste or drop files, ⌘↵ to send")
                    .font(K.F.body).foregroundStyle(K.C.faint)
                    .padding(.horizontal, K.S.sm + 2).padding(.vertical, K.S.sm)
                    .allowsHitTesting(false)
            }

            TextEditor(text: $model.prompt)
                .font(K.F.body)
                .scrollContentBackground(.hidden)
                .padding(.horizontal, K.S.sm).padding(.vertical, K.S.xs + 2)
                .focused($focused)
                .onPasteCommand(of: [.png, .tiff, .fileURL, .plainText]) { _ in
                    if !model.takePaste(.general) {
                        model.prompt += NSPasteboard.general.string(forType: .string) ?? ""
                    }
                }
                // Whatever route a long block took into the box — a paste the text view took
                // before the handler above saw it, a drag, dictation — it leaves as a chip.
                .onChange(of: model.prompt) { if model.prompt.count > SessionModel.longPaste { model.fileLongText() } }
        }
        .frame(height: height)
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

    private var controls: some View {
        HStack(spacing: K.S.sm) {
            ModeToggle(mode: $model.mode)
            Button { model.chooseAttachments() } label: {
                Image(systemName: "paperclip").font(.system(size: 11))
                    .frame(width: 22, height: 20).contentShape(Rectangle())
            }
            .buttonStyle(.plain).foregroundStyle(K.C.faint)
            .hint("Attach files (or paste, drop, or type @)")
            Spacer()
            if model.running {
                Button("Stop") { model.stop() }
                    .buttonStyle(QuietButton(tone: K.C.del))
                    .keyboardShortcut(.escape, modifiers: [])
                    .help("Sends SIGINT — the turn ends rather than being abandoned (⌘. or Esc)")
            } else {
                Button {
                    model.send()
                } label: {
                    HStack(spacing: 5) {
                        Text("Send")
                        Image(systemName: "return").font(.system(size: 10, weight: .bold))
                    }
                }
                .buttonStyle(SendButton())
                .disabled(model.prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
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
            .padding(.horizontal, K.S.sm).padding(.vertical, 3)
            .background(
                RoundedRectangle(cornerRadius: K.R.sm - 1)
                    .fill(on ? K.C.raised : .clear)
                    .padding(1)
            )
            .contentShape(Rectangle())
            .asButton { mode = value }
            .hint("\(label) — \(help) (⇧⇥ switches)")
            .accessibilityAddTraits(on ? .isSelected : [])
    }
}

struct SendButton: ButtonStyle {
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(K.F.small.weight(.medium))
            .foregroundStyle(enabled ? Color.white : K.C.faint)
            .padding(.horizontal, K.S.md).padding(.vertical, 4)
            .background(
                RoundedRectangle(cornerRadius: K.R.sm)
                    .fill(enabled ? K.C.accent.opacity(configuration.isPressed ? 0.75 : 1)
                                  : K.C.line)
            )
    }
}
