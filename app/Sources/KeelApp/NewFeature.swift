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
    @State private var name: String = ""
    @State private var isolated = true
    @State private var provider: SessionModel.Provider = .claude
    @State private var busy = false

    private var branches: [String] { (model.branches?.local ?? []).map(\.name) }
    private var here: String { folder.isEmpty ? model.repoPath : folder }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.lg) {
            VStack(alignment: .leading, spacing: K.S.xxs) {
                Text("New feature").font(K.F.display).foregroundStyle(K.C.text)
                Text("A conversation of its own, and — if you want one — a branch of its own.")
                    .font(K.F.small).foregroundStyle(K.C.dim)
            }

            field("Project") {
                HStack(spacing: K.S.sm) {
                    Text((here as NSString).lastPathComponent.isEmpty ? "—" : (here as NSString).lastPathComponent)
                        .font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                    Text(here).font(K.F.codeTiny).foregroundStyle(K.C.faint)
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

            // The one answer that becomes permanent, and the one the sheet never asked for.
            // The branch was slugged from the first sixty characters of the first message, so
            // typing "hi" made `keel/hi-e8e` and pasting a link made
            // `keel/https-github-com-oyadota-c9e` — names nobody can read a `git log` by, and
            // renaming a lane only ever renamed the tab.
            if isolated {
                field("Name") {
                    TextField("named for your first message", text: $name)
                        .textFieldStyle(.roundedBorder)
                        .font(K.F.small)
                    if !SessionModel.slug(name).isEmpty {
                        Text("Branch `keel/\(SessionModel.slug(name))`")
                            .font(K.F.micro).foregroundStyle(K.C.faint)
                    }
                }
            }

            field("Working tree") {
                VStack(alignment: .leading, spacing: K.S.xs) {
                    Picker("", selection: $isolated) {
                        Text("Its own branch and checkout").tag(true)
                        if !model.policyRequiresIsolation {
                            Text("Share the project's working tree").tag(false)
                        }
                    }
                    .pickerStyle(.radioGroup).labelsHidden()
                    Text(isolated
                         ? "Work here cannot disturb the other features; merge it back when it is done."
                         : "Everything runs in the project itself, which is what you want for reading or reviewing.")
                        .font(K.F.micro).foregroundStyle(K.C.faint)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            field("Agent") {
                Picker("", selection: $provider) {
                    ForEach(SessionModel.Provider.allCases.filter {
                        model.allowedProviders.contains($0.queryValue)
                    }, id: \.self) { provider in
                        Text(provider.rawValue).tag(provider)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
            }

            if isolated {
                field("Branch from") {
                    // Shown even when the project is being changed. It used to be hidden by a
                    // `folder.isEmpty` guard — the branch list belongs to the project that is
                    // open, not the one being chosen — so picking a different project silently
                    // took away the choice this sheet exists to offer instead of saying why.
                    if folder.isEmpty, !branches.isEmpty {
                        Picker("", selection: $branch) {
                            Text("Where the project is now (\(model.branch ?? "HEAD"))").tag("")
                            Divider()
                            ForEach(branches, id: \.self) { b in Text(b).tag(b) }
                        }
                        .labelsHidden().fixedSize()
                    } else {
                        Text(folder.isEmpty
                             ? "This project has no branches yet — the feature starts at HEAD."
                             : "Starts where that project is standing. Open it first to pick a "
                               + "branch.")
                            .font(K.F.micro).foregroundStyle(K.C.faint)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }

            HStack {
                Spacer()
                Button("Cancel") { done() }.buttonStyle(QuietButton())
                Button(busy ? "Starting…" : "Start") { start() }
                    .buttonStyle(FilledButton())
                    .disabled(busy || !SessionModel.Provider.allCases.contains {
                        model.allowedProviders.contains($0.queryValue)
                    })
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(K.S.xl)
        .frame(width: 520)
        .background(K.C.bg)
    }

    private func field(_ label: String, @ViewBuilder _ content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            Text(label.uppercased()).sectionLabel()
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
        let wantsIsolation = model.policyRequiresIsolation ? true : isolated
        let selectedProvider = provider
        let chosen = SessionModel.slug(name)
        let move = folder.isEmpty || folder == model.repoPath ? nil : folder
        done()
        Task {
            if let move {
                await model.open(project: move)
                Recents.remember(move)
                await lanes.refreshShared()
            }
            let lane = lanes.newLane(isolated: wantsIsolation)
            lane.baseBranch = base.isEmpty ? nil : base
            lane.provider = selectedProvider
            if !chosen.isEmpty {
                lane.chosenName = chosen
                lane.title = name.trimmingCharacters(in: .whitespaces)
            }
        }
    }
}
