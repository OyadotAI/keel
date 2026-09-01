import SwiftUI

/// The git controls that more than one surface needs.
///
/// Both of these were `private var`s inside `GitPanel`, which is why the Review pane — the screen
/// that exists for the merge decision — could show neither the branch it was about to merge nor a
/// way to commit what was on it. A person on Review had no path to either, and no way to know the
/// Git panel had them.
///
/// Moved rather than reimplemented: the same bodies, the same `model.commit`, `model.stageAll`,
/// `model.confirmingDiscard`. A second copy of the commit box would have been a second set of
/// disabled-when rules to keep in step.

// MARK: - Where you are

/// The branch, how far it is from its remote, and what to do when there is no remote.
struct BranchStatus: View {
    @Bindable var model: SessionModel
    @State private var addingRemote = false
    @State private var remoteURL = ""

    private var b: Wire.Branches? { model.branches }
    private var busy: Bool { model.gitBusy != nil }
    private var current: Wire.Branch? { b?.local.first { $0.current } }

    var body: some View {
        HStack(spacing: K.S.sm) {
            Image(systemName: "arrow.triangle.branch").font(K.F.micro)
                .foregroundStyle(K.C.accent)
            Text(b?.current ?? model.branch ?? "—")
                .font(K.F.body.weight(.semibold)).foregroundStyle(K.C.text).lineLimit(1)
            Spacer()
            if let c = current, c.upstream != nil {
                if c.ahead > 0 { Pill(text: "\(c.ahead) ↑", tone: .warn) }
                if c.behind > 0 { Pill(text: "\(c.behind) ↓", tone: .accent) }
                if c.ahead == 0 && c.behind == 0 { Pill(text: "IN SYNC", tone: .good) }
            } else if let b, !b.remotes.isEmpty {
                // The project has a remote; this branch has simply never been pushed. Saying
                // "NO REMOTE" and offering to add one here told a connected repository it was
                // not connected — and the first Push sets the upstream by itself.
                Pill(text: "NOT PUSHED", tone: .warn)
            } else if b != nil {
                Pill(text: "NO REMOTE", tone: .neutral)
                // The state a freshly created project is in. Fetch and Push are greyed
                // out until this is answered, so the answer sits beside them.
                Button("Add remote…") { addingRemote = true; remoteURL = "" }
                    .buttonStyle(QuietButton(tone: K.C.accent))
                    .disabled(busy)
                    .popover(isPresented: $addingRemote, arrowEdge: .bottom) {
                        VStack(alignment: .leading, spacing: K.S.sm) {
                            Text("Where should this project be pushed?")
                                .font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                            Text("Create an empty repository on GitHub first, then paste its URL. "
                                 + "It becomes `origin`; the first Push sets the upstream.")
                                .font(K.F.micro).foregroundStyle(K.C.dim)
                                .fixedSize(horizontal: false, vertical: true)
                            TextField("git@github.com:owner/repo.git", text: $remoteURL)
                                .textFieldStyle(.roundedBorder).font(K.F.codeSmall)
                                .onSubmit { addRemote() }
                            HStack {
                                Spacer()
                                Button("Cancel") { addingRemote = false }.buttonStyle(QuietButton())
                                Button("Add") { addRemote() }
                                    .buttonStyle(QuietButton(tone: K.C.accent))
                                    .disabled(remoteURL.trimmingCharacters(in: .whitespaces).isEmpty)
                            }
                        }
                        .padding(K.S.md).frame(width: 360)
                    }
            }
        }
    }

    private func addRemote() {
        let url = remoteURL.trimmingCharacters(in: .whitespaces)
        guard !url.isEmpty else { return }
        addingRemote = false
        Task { await model.remote("add", url: url) }
    }
}

// MARK: - Commit

/// One field, one button. The button commits what is staged when something is, and
/// everything when nothing is — which is what the person meant in both cases.
struct CommitBox: View {
    @Bindable var model: SessionModel
    /// Drawn with its own heading in the Git panel; the Review pane already has one.
    var titled = true
    @State private var message = ""

    private var b: Wire.Branches? { model.branches }
    private var busy: Bool { model.gitBusy != nil }

    var body: some View {
        let staged = b?.staged ?? 0, unstaged = b?.unstaged ?? 0
        let all = staged == 0
        let n = all ? model.changes.count : staged
        return VStack(alignment: .leading, spacing: K.S.xs) {
            if titled { RailHeader("Commit changes", trailing: nil) }
            HStack(spacing: K.S.xs) {
                TextField("What changed, in one line", text: $message)
                    .field().font(K.F.small)
                    .onSubmit { commit(all: all) }
                Button(n == 0 ? "Commit" : "Commit \(n)") { commit(all: all) }
                    .buttonStyle(QuietButton(tone: K.C.accent))
                    .disabled(busy || message.trimmingCharacters(in: .whitespaces).isEmpty || n == 0)
                    .help(all ? "Commits every change" : "Commits the \(staged) staged file\(staged == 1 ? "" : "s")")
            }
            HStack(spacing: K.S.xs) {
                // `branches` arrives a moment after the pane does, and until it has, staged and
                // unstaged are both nought — which read as "0 unstaged" beside a button offering
                // to commit two files. Say what is known instead of a number that is not.
                Text(n == 0 ? "nothing to commit"
                     : b == nil ? "\(n) to commit"
                     : (all ? "\(unstaged) unstaged — all will be committed" : "\(staged) staged · \(unstaged) unstaged"))
                    .font(K.F.micro).foregroundStyle(K.C.faint)
                Spacer()
                if unstaged > 0 {
                    Button("stage all") { Task { await model.stageAll(true) } }
                        .buttonStyle(.plain).font(K.F.micro).foregroundStyle(K.C.accent).disabled(busy)
                }
                if staged > 0 {
                    Button("unstage all") { Task { await model.stageAll(false) } }
                        .buttonStyle(.plain).font(K.F.micro).foregroundStyle(K.C.accent).disabled(busy)
                }
                if n > 0 {
                    // Destructive, so it asks — and the question says what goes where.
                    Button("discard all") { model.confirmingDiscard = true }
                        .buttonStyle(.plain).font(K.F.micro).foregroundStyle(K.C.del).disabled(busy)
                }
            }
        }
        .padding(.horizontal, titled ? K.S.md : 0)
        .padding(.bottom, titled ? K.S.sm : 0)
        // The dialog belongs to the button that raises it. `confirmingDiscard` is a model flag and
        // its dialog was attached four files away in `SidePanel`, so the link above would have
        // done nothing at all from the Review pane — a destructive action that silently does
        // nothing is worse than one that is missing.
        .confirmationDialog("Discard every uncommitted change?",
                            isPresented: $model.confirmingDiscard, titleVisibility: .visible) {
            Button("Discard \(model.changes.count) file\(model.changes.count == 1 ? "" : "s")",
                   role: .destructive) {
                Task { await model.discardAll() }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Modified files go back to the last commit. New files go to the Trash, where they "
                 + "can be recovered. Ignored files such as .env are left alone. The last commit "
                 + "is untouched.")
        }
    }

    private func commit(all: Bool) {
        Task { await model.commit(message, all: all); if model.lastError == nil { message = "" } }
    }
}

// MARK: - Finishing a lane

/// The two alerts that end a lane, and the state behind them.
///
/// Offered from the tab's context menu and from the Review pane, which are two places that must
/// say the same thing about where the work lands — `Lanes.finishBlurb` exists because that
/// sentence was already wrong once for reading the focused lane instead of this one.
@MainActor
@Observable
final class FinishFlow {
    var finishing = false
    var message = ""
    /// The daemon's refusal, which is what the confirmation says. `nil` means nothing is being
    /// confirmed.
    var discarding: String?

    func begin(_ lane: SessionModel) {
        message = lane.title
        finishing = true
    }

    /// Ask the daemon first: it knows what is on the branch and what is not committed at all.
    func askToDiscard(_ lane: SessionModel, _ lanes: Lanes) {
        lanes.activeID = lane.id
        Task {
            if let why = await lanes.discard(lane, force: false) { discarding = why }
        }
    }
}

extension View {
    /// The Finish and Discard alerts, wherever they are raised from.
    ///
    /// `lanes` is optional because the Review pane is also drawn without a window — in tests and
    /// in the visual catalogue. A throwaway `Lanes` per body evaluation would have been the other
    /// way to satisfy the compiler, and it would have allocated one on every frame.
    func finishAndDiscard(_ flow: FinishFlow, lane: SessionModel, lanes: Lanes?,
                          checkout: Wire.Worktree?) -> some View {
        self
            .alert("Finish this task", isPresented: Binding(get: { flow.finishing },
                                                           set: { flow.finishing = $0 })) {
                TextField("Commit message", text: Binding(get: { flow.message },
                                                          set: { flow.message = $0 }))
                Button("Commit and merge") {
                    guard let lanes else { return }
                    Task { await lanes.finishChecked(lane, message: flow.message) }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text(Lanes.finishBlurb(checkout))
            }
            .alert("Discard this feature?",
                   isPresented: Binding(get: { flow.discarding != nil },
                                        set: { if !$0 { flow.discarding = nil } })) {
                Button("Discard anyway", role: .destructive) {
                    // A forced discard that fails used to produce nothing at all — the tab stayed
                    // and nothing was said, which reads as the button not working.
                    guard let lanes else { return }
                    Task {
                        if let why = await lanes.discard(lane, force: true) { lane.lastError = why }
                    }
                }
                Button("Keep it", role: .cancel) {}
            } message: {
                Text(flow.discarding ?? "")
            }
    }
}
