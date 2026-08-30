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
    let lanes: Lanes
    @State private var finishing = false
    @State private var message = ""
    @State private var exported: String?
    @State private var identityOpen = false

    private var packet: ReviewPacket { ReviewPacket(model: model) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                verdict
                identity
                evidence
                files
                commands
                gates
                export
                finish
            }
            .padding(.horizontal, K.S.xl)
            .padding(.bottom, K.S.xxl)
            .frame(maxWidth: 760, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .center)
        }
        .background(K.C.bg)
        .alert("Finish this task", isPresented: $finishing) {
            TextField("Commit message", text: $message)
            Button("Commit and merge") { Task { await lanes.finish(model, message: message) } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This human action merges the isolated task branch into the project.")
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            Text("Review").font(K.F.small.weight(.medium)).foregroundStyle(K.C.accent)
            Text(packet.title).font(K.F.display).foregroundStyle(K.C.text).lineLimit(2)
        }
        .padding(.top, K.S.xl).padding(.bottom, K.S.lg)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var verdict: some View {
        HStack(spacing: K.S.md) {
            Image(systemName: packet.blocker == nil ? "checkmark.seal.fill" : "exclamationmark.shield.fill")
                .font(.system(size: 24, weight: .semibold))
                .foregroundStyle(packet.blocker == nil ? K.C.add : K.C.warn)
            VStack(alignment: .leading, spacing: 2) {
                Text(packet.blocker == nil ? "Ready for your merge decision" : "Not ready to merge")
                    .font(K.F.title).foregroundStyle(K.C.text)
                Text(packet.blocker ?? "Every recorded implementation turn passed its project gate.")
                    .font(K.F.small).foregroundStyle(K.C.dim)
            }
        }
        .padding(K.S.lg)
        .marginBottom(K.S.sm)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background((packet.blocker == nil ? K.C.add : K.C.warn).opacity(0.08), in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke((packet.blocker == nil ? K.C.add : K.C.warn).opacity(0.3)))
    }

    private var identity: some View {
        section("Task details") {
            DisclosureGroup(isExpanded: $identityOpen) {
                VStack(alignment: .leading, spacing: K.S.sm) {
                    fact("Task", packet.title)
                    fact("Provider", packet.provider.rawValue)
                    fact("Task ID", packet.taskID.uuidString.lowercased())
                    fact("Branch", packet.branch ?? "not created")
                    fact("Worktree", packet.worktree ?? "not isolated")
                    fact("Policy", model.policySources.joined(separator: " → "))
                }
                .padding(.top, K.S.sm)
            } label: {
                Text("Branch, provider, worktree, and policy")
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
        section("Commands run") {
            if packet.commands.isEmpty {
                empty("No shell commands recorded")
            } else {
                ForEach(packet.commands) { call in
                    HStack(alignment: .top, spacing: K.S.sm) {
                        Image(systemName: call.failed ? "xmark.circle.fill" : "terminal")
                            .foregroundStyle(call.failed ? K.C.del : K.C.faint)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(call.subject).font(K.F.code).foregroundStyle(K.C.text).lineLimit(2)
                            Text(call.reason ?? "No intent supplied by the agent")
                                .font(K.F.micro).foregroundStyle(call.reason == nil ? K.C.warn : K.C.faint)
                        }
                    }
                }
            }
        }
    }

    private var gates: some View {
        section("Quality evidence") {
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
            if packet.blocker != nil && !model.running {
                Button("Run project checks again") { Task { await model.runGateNow() } }
                    .buttonStyle(QuietButton(tone: K.C.accent))
            }
        }
    }

    @ViewBuilder private var finish: some View {
        if packet.worktree != nil {
            Button {
                message = packet.title
                finishing = true
            } label: {
                Label("Review complete — merge task", systemImage: "arrow.triangle.merge")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(FilledButton())
            .disabled(packet.blocker != nil)
        }
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
        VStack(alignment: .leading, spacing: 1) {
            Text("\(value)").font(K.F.title).foregroundStyle(K.C.text)
            Text(label).font(K.F.micro).foregroundStyle(K.C.faint)
        }
    }

    private func empty(_ text: String) -> some View {
        Text(text).font(K.F.small).foregroundStyle(K.C.faint)
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
