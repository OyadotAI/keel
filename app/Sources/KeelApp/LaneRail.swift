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
                Text("SESSIONS").font(.system(size: 9.5, weight: .semibold)).tracking(0.7)
                    .foregroundStyle(K.C.faint)
                if lanes.runningCount > 0 {
                    Text("\(lanes.runningCount) running")
                        .font(K.F.mono(9.5)).foregroundStyle(K.C.accent)
                }
                Spacer()
                Button { lanes.newLane() } label: {
                    Image(systemName: "plus")
                        .font(.system(size: 10, weight: .bold))
                        .frame(width: 20, height: 20)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain).foregroundStyle(K.C.faint)
                .help("Start another agent alongside this one — useful for reading or planning "
                      + "while one is editing (⌘N)")
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

    private var selected: Bool { lanes.activeID == lane.id }

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
                }
                activityLine
            }
            if hovering && lanes.lanes.count > 1 {
                CloseButton(size: 8) { lanes.close(lane) }
                    .help("Close this lane")
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, 5)
        .background(selected ? K.C.accent.opacity(0.14)
                             : (hovering ? K.C.text.opacity(0.05) : .clear))
        .overlay(alignment: .leading) {
            if selected { Rectangle().fill(K.C.accent).frame(width: 2) }
        }
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture { lanes.activeID = lane.id }
    }

    private var marker: some View {
        Group {
            switch lane.activity {
            case .waiting:
                Image(systemName: "hand.raised.fill")
                    .font(.system(size: 8)).foregroundStyle(K.C.warn)
            case .failed:
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 8)).foregroundStyle(K.C.del)
            case .working:
                StatusDot(failed: false, running: true)
            case .idle:
                Circle().fill(K.C.faint.opacity(0.4)).frame(width: 5, height: 5)
            }
        }
        .frame(width: 10)
        .padding(.top, 4)
    }

    /// One line saying what it is doing, so the rail answers "what is happening" without a click.
    @ViewBuilder
    private var activityLine: some View {
        switch lane.activity {
        case .working(let what):
            HStack(spacing: 4) {
                Sweep().scaleEffect(0.7, anchor: .leading).frame(width: 32, height: 3)
                Text(what).font(K.F.mono(9.5)).foregroundStyle(K.C.accent)
                    .lineLimit(1).truncationMode(.head)
            }
        case .waiting:
            Text("needs you").font(K.F.mono(9.5)).foregroundStyle(K.C.warn)
        case .failed:
            Text("gate failed").font(K.F.mono(9.5)).foregroundStyle(K.C.del)
        case .idle:
            Text(lane.turns.isEmpty ? "empty" : "\(lane.turns.count) turn\(lane.turns.count == 1 ? "" : "s")")
                .font(K.F.mono(9.5)).foregroundStyle(K.C.faint)
        }
    }
}
