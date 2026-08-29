import SwiftUI

/// Every session in this project, side by side, with what each one is doing right now.
///
/// The point of the window. A lane that is waiting on you says so from here without you switching
/// to it, and a lane that failed its gate says so too — which is the difference between running
/// three agents and running one agent three times.
struct LaneRail: View {
    @Bindable var lanes: Lanes

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: K.S.xs) {
                Text("SESSIONS").font(.system(size: 10, weight: .semibold)).tracking(0.7)
                    .foregroundStyle(K.C.faint)
                if lanes.runningCount > 0 {
                    Text("\(lanes.runningCount) running")
                        .font(K.F.mono(10)).foregroundStyle(K.C.accent)
                }
                Spacer()
                // Click for a lane with its own checkout; the menu for one that shares the tree,
                // which is the right shape for reading or reviewing beside an agent that edits.
                Menu {
                    Button("New lane on its own branch") { lanes.newLane(isolated: true) }
                    Button("New lane sharing the working tree") { lanes.newLane() }
                } label: {
                    Image(systemName: "plus")
                        .font(.system(size: 10, weight: .bold))
                        .frame(width: 20, height: 20)
                        .contentShape(Rectangle())
                } primaryAction: {
                    lanes.newLane(isolated: true)
                }
                .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                .foregroundStyle(K.C.faint)
                .hint("New lane in its own checkout (⌘N). The menu offers one that "
                      + "shares the working tree instead.")
            }
            .padding(.horizontal, K.S.md).padding(.top, K.S.md).padding(.bottom, K.S.xs)

            ForEach(lanes.lanes) { lane in
                LaneRow(lane: lane, lanes: lanes)
            }
        }
    }
}

private struct LaneRow: View {
    @Bindable var lane: SessionModel
    let lanes: Lanes
    @State private var hovering = false
    @State private var finishing = false
    @State private var message = ""
    @State private var discarding: String?

    private var selected: Bool { lanes.activeID == lane.id }
    private var checkout: Wire.Worktree? { lanes.worktree(of: lane) }

    var body: some View {
        HStack(alignment: .top, spacing: K.S.sm) {
            marker
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: K.S.xs) {
                    Text(lane.title)
                        .font(K.F.small.weight(selected ? .semibold : .regular))
                        .foregroundStyle(lane.turns.isEmpty ? K.C.faint : K.C.text)
                        .italic(lane.turns.isEmpty)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    if lane.pending.count > 0 {
                        Pill(text: "ASKS", tone: .warn)
                    }
                    // Per lane, so three agents' spend is three numbers before it is one bill.
                    if let cost = lane.sessionCost {
                        Text(String(format: "$%.2f", cost))
                            .font(K.F.mono(10)).monospacedDigit().foregroundStyle(K.C.faint)
                    }
                }
                activityLine
                if let wt = checkout { branchLine(wt) }
            }
            if hovering && lanes.lanes.count > 1 && lane.worktree == nil {
                CloseButton(size: 10) { lanes.close(lane) }
                    .help("Close this lane")
            }
        }
        .contextMenu {
            if lane.worktree != nil {
                Button("Finish lane — merge into \(lanes.active?.branch ?? "the project")…") {
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
        .background(selected ? K.C.accent.opacity(0.14)
                             : (hovering ? K.C.text.opacity(0.05) : .clear))
        .overlay(alignment: .leading) {
            if selected { Rectangle().fill(K.C.accent).frame(width: 2) }
        }
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .asButton { lanes.activeID = lane.id }
        .accessibilityLabel("\(lane.title), \(activityText)")
        .accessibilityAddTraits(selected ? .isSelected : [])
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
        .padding(.top, 4)
    }

    /// Which checkout, and how far it has gone — the line every worktree tool is asked for and
    /// most are missing: where is this lane's work, right now.
    private func branchLine(_ wt: Wire.Worktree) -> some View {
        HStack(spacing: 4) {
            Image(systemName: "arrow.triangle.branch").font(.system(size: 10))
            Text(wt.branch).lineLimit(1).truncationMode(.middle)
            if wt.ahead > 0 { Text("· \(wt.ahead) ahead") }
            if wt.dirty { Text("· edited") }
        }
        .font(K.F.mono(10)).foregroundStyle(K.C.faint)
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

    /// One line saying what it is doing, so the rail answers "what is happening" without a click.
    @ViewBuilder
    private var activityLine: some View {
        switch lane.activity {
        case .working(let what):
            HStack(spacing: 4) {
                Sweep().scaleEffect(0.7, anchor: .leading).frame(width: 32, height: 3)
                Text(what).font(K.F.mono(10)).foregroundStyle(K.C.accent)
                    .lineLimit(1).truncationMode(.head)
            }
        case .waiting:
            Text("needs you").font(K.F.mono(10)).foregroundStyle(K.C.warn)
        case .failed:
            Text("gate failed").font(K.F.mono(10)).foregroundStyle(K.C.del)
        case .idle:
            Text(lane.turns.isEmpty ? "empty" : "\(lane.turns.count) turn\(lane.turns.count == 1 ? "" : "s")")
                .font(K.F.mono(10)).foregroundStyle(K.C.faint)
        }
    }
}
