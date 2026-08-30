import AppKit
import SwiftUI

/// Starting a feature: which repository, and what to branch from.
///
/// A lane is the unit of work here — one conversation, one branch, one checkout — so the two
/// questions a person actually has at that moment are "in which project" and "from which
/// branch". Both used to be assumed: the open project, and wherever it happened to be
/// standing, which meant a feature started off somebody's half-finished branch by accident.
struct NewFeature: View {
    let model: SessionModel
    let lanes: Lanes
    let done: () -> Void

    @State private var folder: String = ""
    @State private var branch: String = ""
    @State private var isolated = true
    @State private var busy = false

    private var branches: [String] { (model.branches?.local ?? []).map(\.name) }
    private var here: String { folder.isEmpty ? model.repoPath : folder }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.lg) {
            VStack(alignment: .leading, spacing: 2) {
                Text("New feature").font(K.F.display).foregroundStyle(K.C.text)
                Text("A conversation of its own, and — if you want one — a branch of its own.")
                    .font(K.F.small).foregroundStyle(K.C.dim)
            }

            field("Project") {
                HStack(spacing: K.S.sm) {
                    Text((here as NSString).lastPathComponent.isEmpty ? "—" : (here as NSString).lastPathComponent)
                        .font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                    Text(here).font(K.F.mono(10)).foregroundStyle(K.C.faint)
                        .lineLimit(1).truncationMode(.head)
                    Spacer()
                    Menu("Change") {
                        ForEach(Recents.paths, id: \.self) { p in
                            Button((p as NSString).lastPathComponent) { folder = p }
                        }
                        Divider()
                        Button("Choose…") { choose() }
                    }
                    .menuStyle(.borderlessButton).fixedSize()
                }
            }

            field("Working tree") {
                VStack(alignment: .leading, spacing: K.S.xs) {
                    Picker("", selection: $isolated) {
                        Text("Its own branch and checkout").tag(true)
                        Text("Share the project's working tree").tag(false)
                    }
                    .pickerStyle(.radioGroup).labelsHidden()
                    Text(isolated
                         ? "Work here cannot disturb the other features; merge it back when it is done."
                         : "Everything runs in the project itself, which is what you want for reading or reviewing.")
                        .font(K.F.micro).foregroundStyle(K.C.faint)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            if isolated, !branches.isEmpty, folder.isEmpty {
                field("Branch from") {
                    Picker("", selection: $branch) {
                        Text("Where the project is now (\(model.branch ?? "HEAD"))").tag("")
                        Divider()
                        ForEach(branches, id: \.self) { b in Text(b).tag(b) }
                    }
                    .labelsHidden().fixedSize()
                }
            }

            HStack {
                Text("The branch is named for your first message.")
                    .font(K.F.micro).foregroundStyle(K.C.faint)
                Spacer()
                Button("Cancel") { done() }.buttonStyle(QuietButton())
                Button(busy ? "Starting…" : "Start") { start() }
                    .buttonStyle(FilledButton())
                    .disabled(busy)
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(K.S.xl)
        .frame(width: 520)
        .background(K.C.bg)
    }

    private func field(_ label: String, @ViewBuilder _ content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            Text(label.uppercased()).font(.system(size: 10, weight: .semibold)).tracking(0.7)
                .foregroundStyle(K.C.faint)
            content()
        }
    }

    private func choose() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.prompt = "Choose"
        if panel.runModal() == .OK, let url = panel.url { folder = url.path }
    }

    private func start() {
        busy = true
        let base = branch
        let wantsIsolation = isolated
        let move = folder.isEmpty || folder == model.repoPath ? nil : folder
        done()
        Task {
            if let move {
                try? await model.openProject(move)
                Recents.remember(move)
                await lanes.refreshShared()
            }
            let lane = lanes.newLane(isolated: wantsIsolation)
            lane.baseBranch = base.isEmpty ? nil : base
        }
    }
}
