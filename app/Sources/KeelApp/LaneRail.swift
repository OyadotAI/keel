import SwiftUI

/// Every session in this project, side by side, with what each one is doing right now.
///
/// The point of the window. A lane that is waiting on you says so from here without you switching
/// to it, and a lane that failed its gate says so too — which is the difference between running
/// three agents and running one agent three times.
struct LaneTabs: View {
    @Bindable var lanes: Lanes

    /// The width the strip actually has, so the tabs can shrink into it rather than run off the
    /// end of it. Reported, not taken: a `GeometryReader` here would take part in the layout it
    /// is trying to measure.
    @State private var strip: Double = 0

    /// A browser shrinks its tabs until they stop fitting and only then scrolls. Below `96` a tab
    /// is two characters and a close button, so that is the floor; past it the strip scrolls.
    /// `44` is the `+` and its padding, kept out of the division so the button stays reachable
    /// instead of being the first thing pushed off the end.
    private var tabWidth: CGFloat {
        min(220, max(96, (strip - 44) / Double(max(lanes.shown.count, 1))))
    }

    var body: some View {
        HStack(spacing: 0) {
            // The project first, then its sessions — the way a browser puts the site before the
            // tabs. This used to be a small menu in the status bar and a column on the left;
            // both were places people did not look for either.
            ProjectMenu(model: lanes.active, prominent: true)
                .padding(.horizontal, K.S.sm)
            Rectangle().fill(K.C.line).frame(width: 1, height: 20)

            // The tabs and the `+` scroll together, so the button sits against the last tab the
            // way a browser's does. They used to be siblings in the outer row, and the scroll
            // view then took whatever width was left over — which squeezed the third tab down to
            // two characters while a full-width "New feature" pill sat beside it.
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(alignment: .bottom, spacing: K.S.hair) {
                    ForEach(lanes.shown) { lane in
                        LaneRow(lane: lane, lanes: lanes, width: tabWidth)
                    }
                    newTab
                }
                .padding(.leading, K.S.xs)
                .padding(.trailing, K.S.sm)
            }
            .onGeometryChange(for: Double.self) { $0.size.width } action: { strip = $0 }

            if lanes.runningCount > 0 {
                Text("\(lanes.runningCount) running")
                    .font(K.F.codeTiny).foregroundStyle(K.C.accent)
                    .padding(.trailing, K.S.md)
            }
        }
        .frame(height: 38)
        // The empty run of the strip asks the same question when clicked: an empty tab strip in a
        // browser makes a tab, and people click it expecting that. Behind the row rather than a
        // sibling in it: as a sibling with `maxWidth: .infinity` it split the row evenly with the
        // scroll view, so half the width went to blank space and the tabs — and the `+` with them
        // — were cut off with room to spare beside them.
        .background {
            Button {
                NotificationCenter.default.post(name: .keelNewLane, object: nil)
            } label: {
                Color.clear.contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .hint("Click for a new feature")
        }
        .background(K.C.surface)
        .overlay(alignment: .bottom) { Hairline() }
    }

    /// A bare `+`, against the last tab.
    ///
    /// It was a labelled pill because testers could not find a plus — but a plus *beside the
    /// tabs*, where every browser puts one, is a different thing from a plus floating in a
    /// toolbar. The label moves to the tooltip and the width goes back to the tabs.
    private var newTab: some View {
        Button {
            NotificationCenter.default.post(name: .keelNewLane, object: nil)
        } label: {
            Image(systemName: "plus")
                .font(K.F.ui(11, .semibold))
                .frame(width: 28, height: 28)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(K.C.dim)
        .padding(.leading, K.S.xxs)
        .padding(.bottom, K.S.tight)
        .hint("New feature — asks which project, what to call it, and which branch to start "
              + "from (⌘N)")
    }
}

/// The agent's own icon, at favicon size.
///
/// The word "Claude" in a pill was four of the tab's characters spent on something an icon says
/// at a glance — and it is the same job a browser gives a favicon. Falls back to the short name
/// rather than to nothing: `Resources.url` returns `nil` when packaging has not copied the file,
/// and a tab with a hole in it is the failure that reads as a broken build.
struct ProviderMark: View {
    let provider: SessionModel.Provider
    var size: CGFloat = 14

    /// Loaded once per provider. A `ForEach` of tabs re-renders on every keystroke in the
    /// composer, and decoding a PNG on each pass is work with a known answer.
    private static let cache = NSCache<NSString, NSImage>()

    private var image: NSImage? {
        let key = provider.iconResource as NSString
        if let hit = Self.cache.object(forKey: key) { return hit }
        guard let url = Resources.url(provider.iconResource, "png"),
              let made = NSImage(contentsOf: url) else { return nil }
        Self.cache.setObject(made, forKey: key)
        return made
    }

    var body: some View {
        if let image {
            Image(nsImage: image)
                .resizable()
                .interpolation(.high)
                .frame(width: size, height: size)
                .clipShape(RoundedRectangle(cornerRadius: size * 0.22))
                .accessibilityLabel(provider.rawValue)
        } else {
            Text(provider.short)
                .font(K.F.tiny.weight(.medium))
                .foregroundStyle(K.C.faint)
                .padding(.horizontal, K.S.tight).padding(.vertical, K.S.hair)
                .background(K.C.ghost, in: RoundedRectangle(cornerRadius: K.R.sm - 1))
        }
    }
}

private struct LaneRow: View {
    @Bindable var lane: SessionModel
    let lanes: Lanes
    /// Handed down rather than chosen here: every tab has to agree, and only the strip knows how
    /// much room there is to share out.
    let width: CGFloat
    @State private var hovering = false
    @State private var flow = FinishFlow()
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
        // Always, even when it is the only lane. It used to stay behind whenever it was, so both
        // windows held the same conversation and both would `claude --resume` the same session id
        // against one transcript. `close` refills an emptied list with a fresh lane, which is what
        // a window with nothing in it should show anyway.
        lanes.close(lane)
    }

    private var selected: Bool { lanes.activeID == lane.id }
    private var checkout: Wire.Worktree? { lanes.worktree(of: lane) }

    var body: some View {
        HStack(spacing: K.S.sm) {
            marker
            // Which agent is behind this tab. Two lanes running different providers looked
            // identical, and the model picker below them offers a different list for each.
            ProviderMark(provider: lane.provider)
            Text(lane.title)
                .font(K.F.small.weight(selected ? .semibold : .regular))
                .foregroundStyle(lane.turns.isEmpty ? K.C.faint : K.C.text)
                .italic(lane.turns.isEmpty)
                .lineLimit(1).truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
            if lane.pending.count > 0 {
                Pill(text: "ASKS", tone: .warn)
            }
            if let wt = checkout {
                Image(systemName: "arrow.triangle.branch").font(K.F.tiny)
                    .foregroundStyle(wt.dirty ? K.C.accent : K.C.faint)
                    .hint(branchText(wt))
            }
            // Close only, and only on the tab you are pointing at or the one you are in — the
            // way a browser does it. The pencil that used to sit here was a third control in a
            // 200pt tab, and renaming already has two ways in: double-click and the menu.
            CloseButton(size: 9, label: closeLabel) { lanes.close(lane) }
                .opacity(hovering || selected ? 0.7 : 0)
                .hint(closeLabel)
        }
        .padding(.horizontal, K.S.sm)
        .frame(width: width, height: 30)
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
        .finishAndDiscard(flow, lane: lane, lanes: lanes, checkout: checkout)
        // A tab shape, not a pill: rounded at the top and square at the bottom, so the selected
        // one reads as continuous with the pane under it. That continuity is the whole reason a
        // browser's tabs are legible at a glance, and a row of floating pills is not.
        .background {
            let shape = UnevenRoundedRectangle(topLeadingRadius: K.R.md,
                                               bottomLeadingRadius: 0,
                                               bottomTrailingRadius: 0,
                                               topTrailingRadius: K.R.md)
            shape.fill(selected || drag != nil ? K.C.bg
                                               : (hovering ? K.C.chrome : .clear))
            if let drag {
                // Accent once releasing would tear it out, so the outcome is visible before the
                // mouse comes up rather than as a window appearing from nowhere.
                shape.stroke(drag.willDetach ? K.C.accent : K.C.lineStrong,
                             lineWidth: drag.willDetach ? 2 : 1)
            } else if selected {
                shape.stroke(K.C.line, lineWidth: 1)
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
        // Last, and it has to be last. Attached higher up the chain this bound to a view whose
        // only hit-testable area was the drawn text and icons — an unselected tab's background is
        // `.clear` until the `contentShape` above claims it — and `asButton` then wrapped the whole
        // thing in a `Button`, which takes the mouse. Right-clicking a tab did nothing.
        .contextMenu {
            Button("Rename…") { newTitle = lane.title; renaming = true }
            Button("Open in a new window") { detach() }
            Divider()
            if lane.worktree != nil {
                Button("Review task and open a pull request…") {
                    lanes.activeID = lane.id
                    NotificationCenter.default.post(name: .keelReviewTask, object: nil)
                }
                Button("Finish task — merge into \(checkout?.base ?? "the project")…") {
                    lanes.activeID = lane.id
                    flow.begin(lane)
                }
                .disabled(!lane.readyToMerge)
                // A greyed-out row that will not say why is the shape of a bug report. The
                // sentence exists; it was only ever rendered in the pull-request sheet.
                if let why = lane.mergeBlocker {
                    Text(why).font(K.F.micro)
                }
                Button("Close tab, keep the branch") { lanes.close(lane) }
                Button("Discard feature…", role: .destructive) {
                    // Focused first. A refusal lands in `lane.lastError`, which renders only in
                    // the *active* lane's composer — so discarding a background lane and being
                    // refused showed nothing at all, anywhere.
                    flow.askToDiscard(lane, lanes)
                }
            } else {
                Button("Close feature") { lanes.close(lane) }
            }
        }
        .help(tooltip)
        .accessibilityLabel("\(lane.title), \(lane.provider.rawValue), \(activityText)")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    /// Closing a lane with a checkout is not discarding it, and the difference is worth the words.
    private var closeLabel: String {
        lane.worktree == nil
            ? "Close this feature"
            : "Close this tab — the branch and its checkout stay"
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
