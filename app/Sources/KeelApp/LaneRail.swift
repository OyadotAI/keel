import SwiftUI

/// Every session in this project, side by side, with what each one is doing right now.
///
/// The point of the window. A lane that is waiting on you says so from here without you switching
/// to it, and a lane that failed its gate says so too — which is the difference between running
/// three agents and running one agent three times.
struct LaneTabs: View {
    @Bindable var lanes: Lanes

    var body: some View {
        HStack(spacing: 0) {
            // The project first, then its sessions — the way a browser puts the site before the
            // tabs. This used to be a small menu in the status bar and a column on the left;
            // both were places people did not look for either.
            ProjectMenu(model: lanes.active, prominent: true)
                .padding(.horizontal, K.S.sm)
            Rectangle().fill(K.C.line).frame(width: 1, height: 20)

            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: K.S.xxs) {
                    ForEach(lanes.lanes.filter { !$0.hidden }) { lane in
                        LaneRow(lane: lane, lanes: lanes)
                    }
                }
                .padding(.horizontal, K.S.sm)
            }

            // A labelled button, not a bare plus: testers did not find the plus. One button,
            // and it opens the same dialog the menu bar and ⌘N open — it used to be a split
            // button whose halves skipped the questions, so the two disagreed about what "new
            // feature" meant.
            Button {
                NotificationCenter.default.post(name: .keelNewLane, object: nil)
            } label: {
                HStack(spacing: K.S.xs) {
                    Image(systemName: "plus").font(K.F.tiny.weight(.bold))
                    Text("New feature").font(K.F.small)
                }
                .padding(.horizontal, K.S.sm).padding(.vertical, K.S.xs)
                .background(K.C.accent.wash, in: RoundedRectangle(cornerRadius: K.R.sm))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(K.C.accent)
            .padding(.leading, K.S.xs)
            .hint("Start another agent — asks which project and which branch to start from (⌘N)")

            // The empty run of the header asks the same question when clicked: an empty tab
            // strip in a browser makes a tab, and people click it expecting that.
            Menu {
                Text("Start a new feature?")
                Button("On its own branch") { lanes.newLane(isolated: true) }
                Button("Sharing the working tree") { lanes.newLane() }
            } label: {
                Color.clear.frame(maxWidth: .infinity, minHeight: 36).contentShape(Rectangle())
            }
            .menuStyle(.borderlessButton).menuIndicator(.hidden)
            .hint("Click for a new feature")
            if lanes.runningCount > 0 {
                Text("\(lanes.runningCount) running")
                    .font(K.F.codeTiny).foregroundStyle(K.C.accent)
                    .padding(.trailing, K.S.md)
            }
        }
        .frame(height: 42)
        .background(K.C.surface)
    }
}

private struct LaneRow: View {
    @Bindable var lane: SessionModel
    let lanes: Lanes
    @State private var hovering = false
    @State private var finishing = false
    @State private var message = ""
    @State private var discarding: String?
    @State private var renaming = false
    @State private var newTitle = ""

    /// How far the tab has been dragged, while it is being dragged.
    ///
    /// `@GestureState` rather than `@State`: it resets itself the moment the gesture ends *or is
    /// cancelled*, so a tab can never be left stuck in the air by a drag that went to another
    /// window or got interrupted. That is the whole reason this is not two `@State` flags.
    @GestureState private var drag: DragState?

    struct DragState: Equatable {
        var translation: CGSize
        /// Past the distance at which releasing tears the tab out into its own window.
        var willDetach: Bool
    }

    /// Vertical distance at which a drag stops being a fidget and becomes "put this in its own
    /// window". Named because two places need to agree on it.
    private static let tearOff: CGFloat = 44

    @Environment(\.openWindow) private var openWindow

    private func detach() {
        // The conversation moves: the new window resumes this session in its own Keel, and the
        // tab leaves this one, the way a browser tab does.
        openWindow(id: "feature", value: Detached(lane: lane.id,
                                                  project: lane.repoPath,
                                                  session: lane.sessionId,
                                                  title: lane.title))
        Telemetry.track("lane_detached", ["resumed": lane.sessionId != nil])
        if lanes.lanes.count > 1 { lanes.close(lane) }
    }

    private var selected: Bool { lanes.activeID == lane.id }
    private var checkout: Wire.Worktree? { lanes.worktree(of: lane) }

    var body: some View {
        HStack(spacing: K.S.sm) {
            marker
            // Which agent is behind this tab. Two lanes running different providers looked
            // identical, and the model picker below them offers a different list for each.
            Text(lane.provider.short)
                .font(K.F.tiny.weight(.medium))
                .foregroundStyle(K.C.faint)
                .padding(.horizontal, K.S.tight).padding(.vertical, K.S.hair)
                .background(K.C.ghost, in: RoundedRectangle(cornerRadius: K.R.sm - 1))
            Text(lane.title)
                .font(K.F.small.weight(selected ? .semibold : .regular))
                .foregroundStyle(lane.turns.isEmpty ? K.C.faint : K.C.text)
                .italic(lane.turns.isEmpty)
                .lineLimit(1).truncationMode(.tail)
                .frame(maxWidth: 200, alignment: .leading)
            if lane.pending.count > 0 {
                Pill(text: "ASKS", tone: .warn)
            }
            if let wt = checkout {
                Image(systemName: "arrow.triangle.branch").font(K.F.tiny)
                    .foregroundStyle(wt.dirty ? K.C.accent : K.C.faint)
                    .hint(branchText(wt))
            }
            Button { newTitle = lane.title; renaming = true } label: {
                Image(systemName: "pencil").font(K.F.tiny)
                    .frame(width: 16, height: 16).contentShape(Rectangle())
            }
            .buttonStyle(.plain).foregroundStyle(K.C.faint)
            .opacity(hovering ? 1 : 0.4)
            .hint("Rename this feature (double-click also works)")
            if lanes.lanes.count > 1 && lane.worktree == nil {
                CloseButton(size: 10, label: "Close this feature") { lanes.close(lane) }
                    .opacity(hovering ? 1 : 0.4)
            }
        }
        .onTapGesture(count: 2) { newTitle = lane.title; renaming = true }
        .alert("Rename feature", isPresented: $renaming) {
            TextField("Name", text: $newTitle)
            Button("Rename") { lane.rename(to: newTitle) }
            Button("Cancel", role: .cancel) {}
        }
        // Dragged out of the strip and released: a window of its own, the way a browser tab
        // works. Two agents side by side is the reason lanes exist.
        //
        // The gesture used to be `onEnded` alone, so holding a tab and moving it did nothing at
        // all — no lift, no movement, no sign it had been picked up — and the window that appeared
        // on release came from nowhere. A dragged tab now behaves like every other dragged tab.
        .gesture(
            DragGesture(minimumDistance: 12, coordinateSpace: .global)
                .updating($drag) { g, state, _ in
                    state = DragState(translation: g.translation,
                                      willDetach: abs(g.translation.height) > Self.tearOff)
                }
                .onEnded { g in
                    if abs(g.translation.height) > Self.tearOff { detach() }
                }
        )
        .contextMenu {
            Button("Rename…") { newTitle = lane.title; renaming = true }
            Button("Open in a new window") { detach() }
            Divider()
            if lane.worktree != nil {
                Button("Review task before merge…") {
                    lanes.activeID = lane.id
                    NotificationCenter.default.post(name: .keelReviewTask, object: nil)
                }
                Button("Finish task — merge into \(lanes.active.branch ?? "the project")…") {
                    message = lane.title
                    finishing = true
                }
                .disabled(!lane.readyToMerge)
                Button("Discard feature…", role: .destructive) {
                    Task {
                        // Ask the daemon first: it knows how many commits are on the branch.
                        if let why = await lanes.discard(lane, force: false) { discarding = why }
                    }
                }
            } else {
                Button("Close feature") { lanes.close(lane) }
            }
        }
        .alert("Finish this task", isPresented: $finishing) {
            TextField("Commit message", text: $message)
            Button("Commit and merge") { Task { await lanes.finish(lane, message: message) } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text(Lanes.finishBlurb(branch: checkout?.branch))
        }
        .alert("Discard this feature?", isPresented: Binding(get: { discarding != nil },
                                                          set: { if !$0 { discarding = nil } })) {
            Button("Discard anyway", role: .destructive) {
                Task { _ = await lanes.discard(lane, force: true) }
            }
            Button("Keep it", role: .cancel) {}
        } message: {
            Text(discarding ?? "")
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
        .background(
            RoundedRectangle(cornerRadius: K.R.md)
                .fill(drag != nil ? K.C.raised
                                  : (selected ? K.C.raised : (hovering ? K.C.chrome : .clear)))
        )
        .overlay {
            if let drag {
                // Accent once releasing would tear it out, so the outcome is visible before the
                // mouse comes up rather than as a window appearing from nowhere.
                RoundedRectangle(cornerRadius: K.R.md)
                    .stroke(drag.willDetach ? K.C.accent : K.C.lineStrong,
                            lineWidth: drag.willDetach ? 2 : 1)
            } else if selected {
                RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line)
            }
        }
        // Picked up: it lifts off the strip and follows the pointer. The one shadow in a window
        // that is otherwise flat by decision, because this is the one thing that genuinely *is*
        // above the surface — and it exists only while the tab is in the air.
        .shadow(color: .black.opacity(drag == nil ? 0 : 0.35),
                radius: drag == nil ? 0 : 10, y: drag == nil ? 0 : 4)
        .scaleEffect(drag == nil ? 1 : 1.03)
        .offset(x: drag?.translation.width ?? 0, y: drag?.translation.height ?? 0)
        // Above its neighbours while it is in the air, or it slides underneath them.
        .zIndex(drag == nil ? 0 : 1)
        .animation(K.M.quick, value: drag == nil)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .asButton { lanes.activeID = lane.id }
        .help(tooltip)
        .accessibilityLabel("\(lane.title), \(lane.provider.rawValue), \(activityText)")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    /// What the row used to show on its second and third lines, now the tooltip.
    private var tooltip: String {
        var parts = [lane.provider.rawValue, activityText]
        if let wt = checkout { parts.append(branchText(wt)) }
        if let cost = lane.sessionCost { parts.append(money(cost, places: 2)) }
        return parts.joined(separator: " · ")
    }

    private func branchText(_ wt: Wire.Worktree) -> String {
        var t = wt.branch
        if wt.ahead > 0 { t += " · \(wt.ahead) ahead" }
        if wt.dirty { t += " · edited" }
        return t
    }

    private var marker: some View {
        Group {
            switch lane.activity {
            case .waiting:
                Image(systemName: "hand.raised.fill")
                    .font(K.F.tiny).foregroundStyle(K.C.warn)
            case .failed:
                Image(systemName: "xmark.circle.fill")
                    .font(K.F.tiny).foregroundStyle(K.C.del)
            case .working:
                StatusDot(failed: false, running: true)
            case .idle:
                Circle().fill(K.C.faint.opacity(0.4)).frame(width: 5, height: 5)
            }
        }
        .frame(width: 10)
    }

    /// The activity as words, for VoiceOver.
    private var activityText: String {
        switch lane.activity {
        case .working(let what): "working: \(what)"
        case .waiting: "needs you"
        case .failed: "gate failed"
        case .idle: lane.turns.isEmpty ? "empty" : "\(lane.turns.count) turns"
        }
    }
}
