import SwiftUI

@MainActor
struct ReviewPacket {
    let taskID: UUID
    let title: String
    let provider: SessionModel.Provider
    let branch: String?
    let worktree: String?
    let files: [String]
    let calls: [Turn.Call]
    let gates: [Turn.Gate]
    let blocker: String?

    init(model: SessionModel) {
        taskID = model.id
        title = model.title
        provider = model.provider
        branch = model.branch
        worktree = model.worktree
        files = model.editedThisSession.map(\.path)
        calls = model.turns.flatMap(\.calls)
        gates = model.turns.filter { $0.didWork && !$0.replayed }.map(\.gate)
        blocker = model.mergeBlocker
    }

    var commands: [Turn.Call] {
        calls.filter { ["Bash", "Shell", "Terminal"].contains($0.tool) }
    }
}

struct ReviewPacketView: View {
    @Bindable var model: SessionModel
    /// The lanes this window holds, so Finish and Discard can act on this one. Optional because
    /// the pane is also drawn in tests and in the visual catalogue, where there is no window.
    var lanes: Lanes?
    @State private var exported: String?
    @State private var flow = FinishFlow()
    @State private var identityOpen = false
    /// Open when something failed: a command that did not work is evidence you should not have to
    /// go looking for.
    @State private var commandsOpen = false

    private var packet: ReviewPacket { ReviewPacket(model: model) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                verdict
                branch
                quality
                identity
                evidence
                files
                commands
                export
            }
            .padding(.horizontal, K.S.xl)
            .padding(.bottom, K.S.xxl)
            .frame(maxWidth: 760, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .center)
        }
        .background(K.C.bg)
        // The branch data this pane never asked for. Without it the status line reads "—" and
        // Push is disabled on a branch that is three commits ahead.
        .task { await model.refreshBranches() }
        .finishAndDiscard(flow, lane: model, lanes: lanes, checkout: checkout)
    }

    private var checkout: Wire.Worktree? { lanes?.worktree(of: model) }

    private var header: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            Text("Review").font(K.F.small.weight(.medium)).foregroundStyle(K.C.accent)
            Text(packet.title).font(K.F.display).foregroundStyle(K.C.text).lineLimit(2)
        }
        .padding(.top, K.S.xl).padding(.bottom, K.S.lg)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Ready, blocked, or merged-without-checks — three states, because two of them read as
    /// success and only one of them is.
    private var verdict: some View {
        let blocked = packet.blocker != nil
        let unverified = !blocked && model.noChecksDeclared
        let tone = blocked ? K.C.warn : (unverified ? K.C.warn : K.C.add)
        return HStack(spacing: K.S.md) {
            Image(systemName: blocked ? "exclamationmark.shield.fill"
                  : (unverified ? "questionmark.diamond.fill" : "checkmark.seal.fill"))
                .font(K.F.display)
                .foregroundStyle(tone)
            VStack(alignment: .leading, spacing: K.S.xxs) {
                // The verdict is about the merge, which is the one thing it gates. Committing,
                // pushing and opening a pull request stay available while it is red — those are
                // how work leaves the machine to be looked at, and this is a warning about it.
                Text(blocked ? "Not ready to merge"
                     : (unverified ? "Nothing here has been checked" : "Ready to merge"))
                    .font(K.F.title).foregroundStyle(K.C.text)
                Text(packet.blocker ?? (unverified
                     ? unverifiedReason
                     : "Every recorded implementation turn passed its project gate."))
                    .font(K.F.small).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(K.S.lg)
        .marginBottom(K.S.sm)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(tone.opacity(0.08), in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(tone.opacity(0.3)))
    }

    /// Why nothing was verified, in the project's own words. Keel never invents a check, so this
    /// is a caveat on the merge rather than a refusal — but it is never silence.
    private var unverifiedReason: String {
        if case .none(let why) = model.latestGate { return why }
        return "This project declares no checks for Keel to run."
    }

    private var identity: some View {
        section("Task details") {
            DisclosureGroup(isExpanded: $identityOpen) {
                VStack(alignment: .leading, spacing: K.S.sm) {
                    fact("Task", packet.title)
                    fact("Provider", packet.provider.rawValue)
                    fact("Task ID", packet.taskID.uuidString.lowercased())
                    fact("Worktree", packet.worktree ?? "shares the project's tree")
                    fact("Policy", model.policySources.joined(separator: " → "))
                }
                .padding(.top, K.S.sm)
            } label: {
                Text("Provider, task id, and policy")
                    .font(K.F.small).foregroundStyle(K.C.dim)
            }
        }
    }

    private var evidence: some View {
        section("Evidence") {
            HStack(spacing: K.S.lg) {
                metric(packet.files.count, "files touched")
                metric(packet.calls.count, "tool calls")
                metric(packet.commands.count, "commands")
                metric(packet.gates.count, "quality runs")
            }
            Text("Approved scope: up to \(model.scopeFileLimit) files")
                .font(K.F.micro).foregroundStyle(K.C.faint)
        }
    }

    @ViewBuilder private var files: some View {
        section("Files touched") {
            if packet.files.isEmpty {
                empty("No implementation files recorded")
            } else {
                ForEach(packet.files, id: \.self) { path in
                    Button { model.show(diff: path) } label: {
                        HStack {
                            Image(systemName: "doc.text").foregroundStyle(K.C.faint)
                            Text(path).font(K.F.code).foregroundStyle(K.C.text).lineLimit(1)
                            Spacer()
                            Image(systemName: "chevron.right").foregroundStyle(K.C.faint)
                        }
                    }.buttonStyle(.plain)
                }
            }
        }
    }

    @ViewBuilder private var commands: some View {
        // Folded, and one line each.
        //
        // A turn that reads a codebase runs thirty commands, each a `cd … && for f in …` long
        // enough to wrap to two lines, each followed by "No intent supplied by the agent" in
        // warning orange. That is a page of scolding between the person and the merge decision
        // this screen exists for — and the commands are evidence you consult, not the verdict.
        //
        // Failures open the section by themselves, because a command that failed is not evidence
        // you have to go looking for.
        section("Commands run", detail: commandsSummary) {
            if packet.commands.isEmpty {
                empty("No shell commands recorded")
            } else {
                DisclosureGroup(isExpanded: $commandsOpen) {
                    VStack(alignment: .leading, spacing: K.S.xs) {
                        ForEach(packet.commands) { call in
                            HStack(alignment: .firstTextBaseline, spacing: K.S.sm) {
                                Image(systemName: call.failed ? "xmark.circle.fill" : "terminal")
                                    .font(K.F.tiny)
                                    .foregroundStyle(call.failed ? K.C.del : K.C.faint)
                                    .accessibilityHidden(true)
                                Text(call.subject)
                                    .font(K.F.codeSmall).foregroundStyle(K.C.text)
                                    .lineLimit(1).truncationMode(.middle)
                                    .textSelection(.enabled)
                                Spacer(minLength: 0)
                                if let reason = call.reason, !reason.isEmpty {
                                    Text(reason).font(K.F.tiny).foregroundStyle(K.C.faint)
                                        .lineLimit(1)
                                }
                            }
                        }
                    }
                    .padding(.top, K.S.xs)
                } label: {
                    Text(commandsOpen ? "Hide the list" : "Show all \(packet.commands.count)")
                        .font(K.F.small).foregroundStyle(K.C.accent)
                }
                .disclosureGroupStyle(.automatic)
                .onAppear { commandsOpen = packet.commands.contains(where: \.failed) }
            }
        }
    }

    /// What the list says without being opened: how many, and whether any failed.
    private var commandsSummary: String? {
        guard !packet.commands.isEmpty else { return nil }
        let failed = packet.commands.count(where: \.failed)
        let n = packet.commands.count
        let ran = "\(n) command\(n == 1 ? "" : "s")"
        return failed > 0 ? "\(ran), \(failed) failed" : ran
    }

    /// Where the work is, and everything that moves it out of the lane.
    ///
    /// It used to be one button — "create pull request" — hidden entirely unless the lane had a
    /// checkout of its own, so a shared lane's Review pane offered nothing at all and said nothing
    /// about why. Finish and Discard existed only on the tab's right-click menu; the branch, its
    /// distance from the remote and the commit box only in the Git panel. This is the screen for
    /// the merge decision, so they are on it.
    private var branch: some View {
        section("Branch") {
            BranchStatus(model: model)
            if let checkout {
                Text("\(checkout.branch) → \(checkout.base ?? "the project's branch")")
                    .font(K.F.codeSmall).foregroundStyle(K.C.dim)
            } else {
                // A shared lane is not a broken lane, and saying nothing was how it read as one.
                Text("Shares the project's working tree — no branch of its own to merge.")
                    .font(K.F.micro).foregroundStyle(K.C.faint)
            }

            CommitBox(model: model, titled: false)
                .padding(.top, K.S.xs)

            HStack(spacing: K.S.sm) {
                if let checkout {
                    Button("Finish — merge into \(checkout.base ?? "the project")…") {
                        lanes?.activeID = model.id
                        flow.begin(model)
                    }
                    .buttonStyle(FilledButton())
                    .disabled(lanes == nil || model.mergeBlocker != nil || model.gitBusy != nil)
                    .help(Lanes.finishBlurb(checkout))
                }
                Button("Push") { Task { await model.remote("push") } }
                    .buttonStyle(QuietButton())
                    .disabled(pushDisabled)
                Button("Open pull request…") { model.sheet = .pr }
                    .buttonStyle(QuietButton(tone: K.C.accent))
                    .disabled(model.branches?.remotes.isEmpty ?? true)
                Spacer()
                if let lanes, checkout != nil {
                    // "Discard the feature" and "discard the changes" are different sizes of
                    // destruction, so both are on screen and neither borrows the other's word:
                    // this one throws the branch away, the one in the commit box above throws
                    // away what has not been committed.
                    Button("Discard feature…") { flow.askToDiscard(model, lanes) }
                        .buttonStyle(QuietButton(tone: K.C.del))
                }
            }
            .padding(.top, K.S.xs)

            if let why = model.mergeBlocker, checkout != nil {
                // A greyed-out button that will not say why is the shape of a bug report.
                Text(why).font(K.F.micro).foregroundStyle(K.C.warn)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var pushDisabled: Bool {
        guard let b = model.branches, !b.remotes.isEmpty else { return true }
        let current = b.local.first { $0.current }
        return !((current?.ahead ?? 0) > 0 || current?.upstream == nil)
    }

    /// The checks, and the button that runs them — always, not only once something has failed.
    ///
    /// The gate is what this page is for. It used to report whatever happened to be recorded and
    /// offer to re-run only when something was already blocking, so a lane whose checks had never
    /// run showed an empty list and a merge you could press.
    private var quality: some View {
        section("Quality evidence", detail: gateDetail) {
            if case .running(let command) = model.latestGate {
                HStack(spacing: K.S.sm) {
                    Sweep().scaleEffect(0.7, anchor: .leading).frame(width: 32, height: 3)
                    Text(command).font(K.F.codeSmall).foregroundStyle(K.C.dim).lineLimit(1)
                }
            }
            if packet.gates.isEmpty { empty("No project quality gate has run") }
            ForEach(Array(packet.gates.enumerated()), id: \.offset) { index, gate in
                HStack(spacing: K.S.sm) {
                    Image(systemName: gatePassed(gate) ? "checkmark.circle.fill" : "xmark.circle.fill")
                        .foregroundStyle(gatePassed(gate) ? K.C.add : K.C.del)
                    Text("Implementation turn \(index + 1)").font(K.F.small).foregroundStyle(K.C.text)
                    Spacer()
                    Text(gateLabel(gate)).font(K.F.micro).foregroundStyle(K.C.dim)
                }
            }
            if !model.running, !model.isRunningGate {
                Button(model.gateIsCurrent ? "Run project checks again" : "Run project checks") {
                    Task { await model.runGateNow() }
                }
                .buttonStyle(QuietButton(tone: K.C.accent))
                .help("Finish runs these itself; this is for looking before you decide.")
            }
        }
    }

    /// Whether the recorded verdict is about the tree as it stands — said in the header, because
    /// a stale pass and a fresh one look identical in a list.
    private var gateDetail: String? {
        if model.isRunningGate { return "running" }
        if model.noChecksDeclared { return "no checks declared" }
        if model.gateIsCurrent { return "current" }
        return model.changes.isEmpty ? nil : "stale — \(model.changes.count) uncommitted"
    }

    private var export: some View {
        section("Shareable packet") {
            Text("The signed packet contains review evidence, not source code, diffs, prompts or command output.")
                .font(K.F.small).foregroundStyle(K.C.dim)
            HStack {
                Button("Export signed packet") {
                    do { exported = try PacketStore.save(model: model).path }
                    catch { model.lastError = error.localizedDescription }
                }
                .buttonStyle(QuietButton(tone: K.C.accent))
                if let exported {
                    Text(exported).font(K.F.micro).foregroundStyle(K.C.faint)
                        .lineLimit(1).truncationMode(.middle).textSelection(.enabled)
                }
            }
        }
    }

    private func section<Content: View>(_ title: String, detail: String? = nil,
                                        @ViewBuilder content: () -> Content) -> some View {
        ContentSection(title, detail: detail, content: content)
    }

    private func fact(_ name: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(name).font(K.F.small).foregroundStyle(K.C.faint).frame(width: 72, alignment: .leading)
            Text(value).font(K.F.code).foregroundStyle(K.C.text).textSelection(.enabled)
        }
    }

    private func metric(_ value: Int, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: K.S.hair) {
            Text("\(value)").font(K.F.title).foregroundStyle(K.C.text)
            Text(label).font(K.F.micro).foregroundStyle(K.C.faint)
        }
    }

    private func empty(_ text: String) -> some View {
        EmptyState(text).padding(.vertical, -K.S.sm)
    }

    private func gatePassed(_ gate: Turn.Gate) -> Bool {
        if case .passed = gate { return true }
        return false
    }

    private func gateLabel(_ gate: Turn.Gate) -> String {
        switch gate {
        case .notRun: "not run"
        case .running: "running"
        case .passed(let command, _): command
        case .failed(let command, _): "failed · \(command)"
        case .none(let reason): reason
        }
    }
}

private extension View {
    func marginBottom(_ amount: CGFloat) -> some View { padding(.bottom, amount) }
}
