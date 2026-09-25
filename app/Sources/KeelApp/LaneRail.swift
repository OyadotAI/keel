import SwiftUI

/// Every session in this project, side by side, with what each one is doing right now.
///
/// The point of the window. A lane that is waiting on you says so from here without you switching
/// to it, and a lane that failed its gate says so too — which is the difference between running
/// three agents and running one agent three times.
struct LaneTabs: View {
    @Bindable var lanes: Lanes
    var embedded = false
    var showsProject = true

    /// The width the strip actually has, so the tabs can shrink into it rather than run off the
    /// end of it. Reported, not taken: a `GeometryReader` here would take part in the layout it
    /// is trying to measure.
    @State private var strip: Double = 0
    @State private var windowWidth: Double = 0

    /// Tabs retain a readable label and scroll once the strip runs out of room.
    /// The new-session action remains outside the scroll view, reachable at every width.
    private var tabWidth: CGFloat {
        min(240, max(160, strip / Double(max(lanes.shown.count, 1))))
    }

    var body: some View {
        HStack(spacing: 0) {
            // The project first, then its sessions — the way a browser puts the site before the
            // tabs. This used to be a small menu in the status bar and a column on the left;
            // both were places people did not look for either.
            if showsProject {
                ProjectMenu(model: lanes.active, prominent: true)
                    .padding(.horizontal, K.S.sm)
                Rectangle().fill(K.C.line).frame(width: 1, height: 20)
            }

            ScrollView(.horizontal, showsIndicators: false) {
                HStack(alignment: .bottom, spacing: K.S.hair) {
                    ForEach(lanes.shown) { lane in
                        LaneRow(lane: lane, lanes: lanes, width: tabWidth)
                    }
                }
                .padding(.leading, K.S.xs)
                .padding(.trailing, K.S.sm)
            }
            .onGeometryChange(for: Double.self) { $0.size.width } action: { strip = $0 }

            newTab.padding(.horizontal, K.S.xs)
            if windowWidth >= 1000 { LaneCounter(lanes: lanes) }
        }
        .frame(height: 52)
        .onGeometryChange(for: Double.self) { $0.size.width } action: { windowWidth = $0 }
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
        // The title bar's own material, run up under the toolbar (whose background the window
        // hides), so the tabs and the title read as one surface — the way Safari's tab bar does.
        // Static chrome: nothing here scrolls vertically or streams.
        .background { if !embedded { VisualEffect(.titlebar).ignoresSafeArea(edges: .top) } }
        .overlay(alignment: .bottom) { if !embedded { Hairline() } }
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
        openWindow(id: "feature", value: Detached(model: lane))
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
            // The one you are in is named at row size; the rest are quieter unless they need you.
            Text(lane.title)
                .font(selected ? K.F.row : K.F.small)
                .foregroundStyle(title)
                .italic(lane.turns.isEmpty)
                .lineLimit(1).truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
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
        .frame(width: width, height: 36)
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
        // A compact floating tab keeps the unified command band visually continuous.
        .background {
            let shape = RoundedRectangle(cornerRadius: K.R.md)
            shape.fill(selected || drag != nil ? K.C.raised
                       : lane.activity == .waiting ? K.C.warn.wash
                       : (hovering ? K.C.chrome : .clear))
            // A lane that needs you, or whose gate failed, says so from across the room: an edge
            // in the state's colour along the top of the tab, selected or not.
            if let edge {
                Rectangle().fill(edge).frame(height: 2)
                    .frame(maxHeight: .infinity, alignment: .top)
                    .clipShape(shape)
            }
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
        .shadow(color: drag == nil ? .clear : K.C.shadow,
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
            bulkClose
        }
        .help(tooltip)
        .accessibilityLabel("\(lane.title), \(lane.provider.rawValue), \(activityText)")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    /// The browser's tab menu, because a row of tabs sets that expectation and this is a row of
    /// tabs.
    ///
    /// Counted rather than named — "Close 3 to the right" says what will happen where "Close tabs
    /// to the right" leaves you counting them yourself — and each is hidden rather than disabled
    /// when there is nothing to close, so the menu is as long as it has reason to be.
    ///
    /// None of these is destructive. Closing a lane keeps its branch and its checkout, which the
    /// item above says in as many words; throwing work away is `Discard feature…`, and it stays
    /// one lane at a time on purpose.
    @ViewBuilder private var bulkClose: some View {
        let others = lanes.others(than: lane)
        let before = lanes.before(lane)
        let after = lanes.after(lane)
        if !others.isEmpty {
            Divider()
            Button("Close \(others.count) other tab\(others.count == 1 ? "" : "s")") {
                lanes.activeID = lane.id
                lanes.close(others)
            }
            if !before.isEmpty {
                Button("Close \(before.count) to the left") { lanes.close(before) }
            }
            if !after.isEmpty {
                Button("Close \(after.count) to the right") { lanes.close(after) }
            }
            Button("Close all \(others.count + 1) tabs") { lanes.close(lanes.shown) }
        }
    }

    private var edge: Color? {
        switch lane.activity {
        case .waiting: K.C.warn
        case .failed: K.C.del
        default: nil
        }
    }

    private var title: Color {
        if lane.activity == .waiting || lane.activity == .failed || selected {
            return lane.turns.isEmpty ? K.C.dim : K.C.text
        }
        return K.C.dim
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
                // Nothing: a dot on every idle tab was the noise the three real states had to
                // be read through.
                Color.clear
            }
        }
        .frame(width: 10)
    }

    /// The activity as words, for VoiceOver.
    private var activityText: String {
        switch lane.activity {
        case .working: "working"
        case .waiting: "needs you"
        case .failed: "gate failed"
        case .idle: lane.turns.isEmpty ? "empty" : "\(lane.turns.count) turns"
        }
    }
}

/// "1 needs you · 2 running" at the end of the strip. The waiting half is loud and is a button:
/// with the strip scrolled a waiting tab can be out of sight, and this is all that says so.
private struct LaneCounter: View {
    let lanes: Lanes

    var body: some View {
        let waiting = lanes.shown.count { !$0.pending.isEmpty }
        let running = lanes.runningCount
        if waiting > 0 || running > 0 {
            HStack(spacing: K.S.xs) {
                if waiting > 0 {
                    Button {
                        if let lane = lanes.firstWaiting { lanes.activeID = lane.id }
                    } label: {
                        Text("\(waiting) needs you").font(K.F.micro.weight(.semibold))
                            .foregroundStyle(K.C.warn)
                    }
                    .buttonStyle(.plain)
                    .hint("Go to the feature that is waiting on you")
                }
                if waiting > 0 && running > 0 {
                    Text("·").font(K.F.micro).foregroundStyle(K.C.faint)
                }
                if running > 0 {
                    Text("\(running) running").font(K.F.codeTiny).foregroundStyle(K.C.accent)
                }
            }
            .padding(.trailing, K.S.md)
        }
    }
}
