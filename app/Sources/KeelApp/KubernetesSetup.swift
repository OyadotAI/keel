import SwiftUI

/// A context choice is not a successful login or proof of cluster reachability.
@Observable @MainActor
final class KubernetesSetup {
    struct Context: Decodable, Identifiable, Equatable {
        struct Target: Decodable, Equatable { var cluster: String; var namespace: String }
        var name: String
        var context: Target
        var id: String { name }
    }
    struct Snapshot: Decodable {
        var current: String?
        var contexts: [Context]?
        enum CodingKeys: String, CodingKey { case current = "current-context", contexts }
    }
    private let client: Client
    var contexts: [Context] = []
    var current: String?
    var selected: String?
    var loading = false
    var applying = false
    var loaded = false
    var failure: String?
    var confirmation: String?

    init(client: Client, snapshot: Snapshot? = nil) {
        self.client = client
        if let snapshot { accept(snapshot); loaded = true }
    }

    var canApply: Bool {
        !loading && !applying && selected != current
            && contexts.contains { $0.name == selected }
    }

    private func accept(_ snapshot: Snapshot) {
        contexts = snapshot.contexts ?? []
        current = snapshot.current
        if !contexts.contains(where: { $0.name == selected }) {
            selected = contexts.first(where: { $0.name == current })?.name
        }
    }

    func refresh() async {
        guard !loading && !applying else { return }
        loading = true
        failure = nil
        defer { loading = false }
        do {
            let snapshot: Snapshot = try await client.get("/api/kubernetes/contexts")
            accept(snapshot)
            loaded = true
        } catch {
            failure = "Could not load contexts. \(error.localizedDescription)"
        }
    }

    func apply() async {
        guard canApply, let name = selected else { return }
        applying = true
        failure = nil
        confirmation = nil
        defer { applying = false }
        do {
            let snapshot: Snapshot = try await client.post("/api/kubernetes/contexts", body: ["name": name])
            accept(snapshot)
            guard current == name else {
                failure = "The context was not saved. Refresh and try again."
                return
            }
            confirmation = "Default context set to \(name). Cluster connectivity is checked separately in Tools."
        } catch {
            failure = "Could not change context. \(error.localizedDescription)"
        }
    }
}

struct KubernetesContextSheet: View {
    @State private var setup: KubernetesSetup
    @State private var query = ""
    let done: () -> Void

    init(client: Client, setup: KubernetesSetup? = nil, done: @escaping () -> Void) {
        _setup = State(initialValue: setup ?? KubernetesSetup(client: client))
        self.done = done
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            HStack {
                VStack(alignment: .leading, spacing: K.S.xs) {
                    Text("Kubernetes context").font(K.F.title)
                    Text("Choose where kubectl sends commands.").font(K.F.small).foregroundStyle(K.C.dim)
                }
                Spacer()
                CloseButton(size: 10, label: "Close context chooser", action: done)
                    .disabled(setup.applying)
            }

            if setup.loading {
                HStack { ProgressView().controlSize(.small); Text("Loading contexts…").font(K.F.small) }
            }
            if let failure = setup.failure {
                Label(failure, systemImage: "exclamationmark.triangle")
                    .font(K.F.small).foregroundStyle(K.C.del)
                    .fixedSize(horizontal: false, vertical: true)
            }

            if setup.loaded && setup.contexts.isEmpty {
                VStack(alignment: .leading, spacing: K.S.sm) {
                    Text("No contexts configured").font(K.F.body.weight(.semibold))
                    Text("Get a kubeconfig from your cluster provider or administrator, then refresh. Installing kubectl alone does not create a cluster connection.")
                        .font(K.F.small).foregroundStyle(K.C.dim).fixedSize(horizontal: false, vertical: true)
                    Link("How to configure cluster access ↗", destination: URL(string: "https://kubernetes.io/docs/tasks/access-application-cluster/configure-access-multiple-clusters/")!)
                        .font(K.F.small)
                }.padding(.vertical, K.S.md)
            } else if !setup.contexts.isEmpty {
                TextField("Filter by context or cluster", text: $query).field()
                    .accessibilityLabel("Filter contexts")
                let visible = setup.contexts.filter {
                    query.isEmpty || $0.name.localizedCaseInsensitiveContains(query)
                        || $0.context.cluster.localizedCaseInsensitiveContains(query)
                }
                ScrollView {
                    VStack(alignment: .leading, spacing: K.S.xs) {
                        if visible.isEmpty {
                            Text("No matching contexts. Try another search.")
                                .font(K.F.small).foregroundStyle(K.C.dim).padding(K.S.md)
                        }
                        ForEach(visible) { context in
                            Button {
                                setup.selected = context.name
                                setup.confirmation = nil
                            } label: {
                                HStack(alignment: .top, spacing: K.S.sm) {
                                    Image(systemName: setup.selected == context.name ? "largecircle.fill.circle" : "circle")
                                        .foregroundStyle(setup.selected == context.name ? K.C.accent : K.C.dim)
                                    VStack(alignment: .leading, spacing: K.S.xxs) {
                                        Text(context.name).font(K.F.small.weight(.semibold))
                                            .fixedSize(horizontal: false, vertical: true)
                                        Text("\(context.context.cluster) · namespace: \(context.context.namespace.isEmpty ? "default" : context.context.namespace)")
                                            .font(K.F.micro).foregroundStyle(K.C.dim)
                                            .fixedSize(horizontal: false, vertical: true)
                                    }
                                    Spacer(minLength: K.S.sm)
                                    if setup.current == context.name { Pill(text: "CURRENT", tone: .neutral) }
                                }
                                .padding(K.S.sm).frame(maxWidth: .infinity, alignment: .leading)
                                .background(setup.selected == context.name ? K.C.accent.opacity(0.08) : K.C.raised,
                                            in: RoundedRectangle(cornerRadius: K.R.md))
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("\(context.name), cluster \(context.context.cluster)")
                            .accessibilityValue(setup.selected == context.name ? "Selected" : "Not selected")
                            .disabled(setup.loading || setup.applying)
                        }
                    }
                }.frame(height: 250)
            }

            if let confirmation = setup.confirmation {
                Label(confirmation, systemImage: "checkmark.circle")
                    .font(K.F.small).foregroundStyle(K.C.add).fixedSize(horizontal: false, vertical: true)
            }
            Text("Applying changes kubectl’s default context on this Mac, including other terminals and agents. It does not create or modify cluster resources.")
                .font(K.F.small).foregroundStyle(K.C.dim).fixedSize(horizontal: false, vertical: true)
            HStack {
                Button("Refresh") { Task { await setup.refresh() } }
                    .buttonStyle(QuietButton()).disabled(setup.loading || setup.applying)
                Spacer()
                Button("Done", action: done).buttonStyle(QuietButton()).disabled(setup.applying)
                Button(setup.applying ? "Applying…" : "Use selected context") { Task { await setup.apply() } }
                    .buttonStyle(FilledButton()).disabled(!setup.canApply)
            }
        }
        .padding(K.S.xl).frame(width: 600).background(K.C.bg)
        .foregroundStyle(K.C.text)
        .task { if !setup.loaded { await setup.refresh() } }
        .interactiveDismissDisabled(setup.applying)
    }
}
