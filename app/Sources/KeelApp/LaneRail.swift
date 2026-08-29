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
                HStack(spacing: 2) {
                    ForEach(lanes.lanes.filter { !$0.hidden }) { lane in
                        LaneRow(lane: lane, lanes: lanes)
                    }
                }
                .padding(.horizontal, K.S.xs)
            }

            // A labelled button, not a bare plus: testers did not find the plus. Click for a
            // session with its own checkout; the chevron offers one that shares the tree, which
            // is the right shape for reading or reviewing beside an agent that edits.
            Menu {
                Button("New session on its own branch") { lanes.newLane(isolated: true) }
                Button("New session sharing the working tree") { lanes.newLane() }
            } label: {
                HStack(spacing: 4) {
                    Image(systemName: "plus").font(.system(size: 10, weight: .bold))
                    Text("New session").font(K.F.small)
                    Image(systemName: "chevron.down").font(.system(size: 10, weight: .bold))
                }
                .padding(.horizontal, K.S.sm).padding(.vertical, 4)
                .background(K.C.accent.opacity(0.12), in: RoundedRectangle(cornerRadius: K.R.sm))
                .contentShape(Rectangle())
            } primaryAction: {
                lanes.newLane(isolated: true)
            }
            .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
            .foregroundStyle(K.C.accent)
            .padding(.leading, K.S.xs)
            .hint("Start another agent in its own checkout (⌘N). The chevron offers one that "
                  + "shares the working tree instead.")

            // The empty run of the header asks the same question when clicked: an empty tab
            // strip in a browser makes a tab, and people click it expecting that.
            Menu {
                Text("Start a new session?")
                Button("On its own branch") { lanes.newLane(isolated: true) }
                Button("Sharing the working tree") { lanes.newLane() }
            } label: {
                Color.clear.frame(maxWidth: .infinity, minHeight: 36).contentShape(Rectangle())
            }
            .menuStyle(.borderlessButton).menuIndicator(.hidden)
            .hint("Click for a new session")
            if lanes.runningCount > 0 {
                Text("\(lanes.runningCount) running")
                    .font(K.F.mono(10)).foregroundStyle(K.C.accent)
                    .padding(.trailing, K.S.md)
            }
        }
        .frame(height: 36)
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

    private var selected: Bool { lanes.activeID == lane.id }
    private var checkout: Wire.Worktree? { lanes.worktree(of: lane) }

    var body: some View {
        HStack(spacing: K.S.sm) {
            marker
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
                Image(systemName: "arrow.triangle.branch").font(.system(size: 10))
                    .foregroundStyle(wt.dirty ? K.C.accent : K.C.faint)
                    .help(branchText(wt))
            }
            if hovering {
                Button { newTitle = lane.title; renaming = true } label: {
                    Image(systemName: "pencil").font(.system(size: 10))
                        .frame(width: 16, height: 16).contentShape(Rectangle())
                }
                .buttonStyle(.plain).foregroundStyle(K.C.faint)
                .hint("Rename this lane (double-click also works)")
            }
            if hovering && lanes.lanes.count > 1 && lane.worktree == nil {
                CloseButton(size: 10) { lanes.close(lane) }
                    .help("Close this lane")
            }
        }
        .onTapGesture(count: 2) { newTitle = lane.title; renaming = true }
        .alert("Rename lane", isPresented: $renaming) {
            TextField("Name", text: $newTitle)
            Button("Rename") { lane.rename(to: newTitle) }
            Button("Cancel", role: .cancel) {}
        }
        .contextMenu {
            Button("Rename…") { newTitle = lane.title; renaming = true }
            Divider()
            if lane.worktree != nil {
                Button("Finish lane — merge into \(lanes.active.branch ?? "the project")…") {
                    message = lane.title
                    finishing = true
                }
                Button("Discard lane…", role: .destructive) {
                    Task {
                        // Ask the daemon first: it knows how many commits are on the branch.
                        if let why = await lanes.discard(lane, force: false) { discarding = why }
                    }
                }
            } else {
                Button("Close lane") { lanes.close(lane) }
            }
        }
        .alert("Finish this lane", isPresented: $finishing) {
            TextField("Commit message", text: $message)
            Button("Commit and merge") { Task { await lanes.finish(lane, message: message) } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Commits everything in the lane and merges \(checkout?.branch ?? "its branch") "
                 + "into the project. The lane's checkout is removed; the branch is deleted only "
                 + "once it is merged.")
        }
        .alert("Discard this lane?", isPresented: Binding(get: { discarding != nil },
                                                          set: { if !$0 { discarding = nil } })) {
            Button("Discard anyway", role: .destructive) {
                Task { _ = await lanes.discard(lane, force: true) }
            }
            Button("Keep it", role: .cancel) {}
        } message: {
            Text(discarding ?? "")
        }
        .padding(.horizontal, K.S.md).padding(.vertical, 5)
        .background(
            RoundedRectangle(cornerRadius: K.R.sm)
                .fill(selected ? K.C.bg : (hovering ? K.C.text.opacity(0.05) : .clear))
        )
        .overlay(alignment: .bottom) {
            if selected { Rectangle().fill(K.C.accent).frame(height: 2).padding(.horizontal, K.S.xs) }
        }
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .asButton { lanes.activeID = lane.id }
        .help(tooltip)
        .accessibilityLabel("\(lane.title), \(activityText)")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    /// What the row used to show on its second and third lines, now the tooltip.
    private var tooltip: String {
        var parts = [activityText]
        if let wt = checkout { parts.append(branchText(wt)) }
        if let cost = lane.sessionCost { parts.append(String(format: "$%.2f", cost)) }
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
                    .font(.system(size: 10)).foregroundStyle(K.C.warn)
            case .failed:
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 10)).foregroundStyle(K.C.del)
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
