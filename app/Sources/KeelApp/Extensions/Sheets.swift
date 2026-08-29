import SwiftUI

/// Installing a skill means installing the plugin that carries it, which is what `claude` itself
/// does — so this browses the marketplaces already configured rather than inventing a registry.
struct SkillCatalog: View {
    let client: Client
    let done: () -> Void

    @State private var suggested: [Entry] = []
    @State private var all: [Entry] = []
    @State private var query = ""
    @State private var installing: String?
    @State private var log = ""

    struct Catalog: Decodable { var suggested: [Entry]; var all: [Entry] }
    struct Entry: Decodable, Identifiable, Sendable {
        var name: String
        var description: String?
        var marketplace: String
        var installed: Bool
        var reason: String?
        var id: String { marketplace + name }
    }

    private var hits: [Entry] {
        guard !query.isEmpty else { return suggested }
        return all.filter {
            $0.name.localizedCaseInsensitiveContains(query)
                || ($0.description ?? "").localizedCaseInsensitiveContains(query)
        }
        .prefix(60).map { $0 }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "magnifyingglass").font(.system(size: 11))
                    .foregroundStyle(K.C.faint)
                TextField("Search \(all.count) skills and plugins…", text: $query)
                    .textFieldStyle(.plain).font(K.F.body)
                Button("Done") { done() }.buttonStyle(QuietButton())
            }
            .padding(K.S.md)
            Hairline()

            if query.isEmpty && !suggested.isEmpty {
                Text("SUGGESTED FOR THIS REPOSITORY")
                    .font(.system(size: 10, weight: .semibold)).tracking(0.7)
                    .foregroundStyle(K.C.faint)
                    .padding(.horizontal, K.S.md).padding(.top, K.S.sm)
            }

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(hits) { e in row(e) }
                }
            }
            .frame(height: 340)

            if !log.isEmpty {
                Hairline()
                ScrollView {
                    Text(log).font(K.F.codeSmall).foregroundStyle(K.C.dim)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(K.S.sm)
                }
                .frame(height: 90)
                .background(K.C.well)
            }
        }
        .frame(width: 560)
        .background(K.C.bg)
        .task {
            if let c: Catalog = try? await client.get("/api/plugins") {
                suggested = c.suggested
                all = c.all
            }
        }
    }

    private func row(_ e: Entry) -> some View {
        HStack(alignment: .top, spacing: K.S.sm) {
            VStack(alignment: .leading, spacing: K.S.xxs) {
                HStack(spacing: K.S.xs) {
                    Text(e.name).font(K.F.small.weight(.medium)).foregroundStyle(K.C.text)
                    Text(e.marketplace).font(K.F.mono(10)).foregroundStyle(K.C.faint)
                }
                // A recommendation is a row that has to argue for itself.
                if let why = e.reason, !why.isEmpty {
                    Text(why).font(K.F.micro).foregroundStyle(K.C.accent)
                        .fixedSize(horizontal: false, vertical: true)
                } else if let d = e.description, !d.isEmpty {
                    Text(d).font(K.F.micro).foregroundStyle(K.C.dim)
                        .lineLimit(2).fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: K.S.sm)
            if e.installed {
                Text("installed").font(K.F.micro).foregroundStyle(K.C.add)
            } else {
                Button(installing == e.id ? "…" : "Install") { install(e) }
                    .buttonStyle(QuietButton())
                    .disabled(installing != nil)
            }
        }
        .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
    }

    private func install(_ e: Entry) {
        installing = e.id
        log = ""
        Task {
            defer { installing = nil }
            do {
                for try await ev in client.events("/api/plugins/install",
                                                  ["name": e.name, "marketplace": e.marketplace]) {
                    switch ev.name {
                    case "line", "fatal": log += ev.data + "\n"
                    case "done":
                        if let c: Catalog = try? await client.get("/api/plugins") {
                            suggested = c.suggested
                            all = c.all
                        }
                    default: break
                    }
                }
            } catch { log += error.localizedDescription }
        }
    }
}

struct AddMCP: View {
    let client: Client
    let done: () -> Void

    @State private var name = ""
    @State private var transport = "http"
    @State private var target = ""
    @State private var scope = "local"
    @State private var log = ""

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            Text("Add an MCP server").font(K.F.title).foregroundStyle(K.C.text)
            field("Name", "linear", $name)
            Picker("Transport", selection: $transport) {
                Text("HTTP").tag("http"); Text("SSE").tag("sse"); Text("stdio").tag("stdio")
            }
            .pickerStyle(.segmented).labelsHidden()
            field(transport == "stdio" ? "Command" : "URL",
                  transport == "stdio" ? "npx -y some-server" : "https://…", $target)
            Picker("Scope", selection: $scope) {
                Text("This machine").tag("local")
                Text("All my projects").tag("user")
            }
            .pickerStyle(.segmented).labelsHidden()

            // `project` is deliberately absent: writing a server into the repository configures a
            // command to run on the machine of whoever clones it next.
            Text("Keel never writes a server into the repository — that would be configuring a "
                 + "command to run on someone else's machine.")
                .font(K.F.micro).foregroundStyle(K.C.dim)
                .fixedSize(horizontal: false, vertical: true)

            if !log.isEmpty {
                Text(log).font(K.F.codeSmall).foregroundStyle(K.C.dim).lineLimit(6)
            }

            HStack {
                Spacer()
                Button("Cancel") { done() }.buttonStyle(QuietButton())
                Button("Add") { add() }
                    .buttonStyle(SendButtonWide())
                    .disabled(name.isEmpty || target.isEmpty)
            }
        }
        .padding(K.S.xl).frame(width: 440).background(K.C.bg)
    }

    private func field(_ label: String, _ hint: String, _ value: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(label).font(K.F.micro).foregroundStyle(K.C.faint)
            TextField(hint, text: value)
                .textFieldStyle(.plain).font(K.F.code)
                .padding(.horizontal, K.S.sm).padding(.vertical, 5)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
        }
    }

    private func add() {
        Task {
            do {
                for try await e in client.events("/api/mcp/add", [
                    "name": name, "transport": transport, "target": target, "scope": scope,
                ]) {
                    switch e.name {
                    case "line", "fatal": log += e.data + "\n"
                    case "done": done()
                    default: break
                    }
                }
            } catch { log += error.localizedDescription }
        }
    }
}

struct NewSubagent: View {
    let client: Client
    let done: () -> Void

    @State private var name = ""
    @State private var about = ""
    @State private var prompt = ""
    @State private var error: String?

    struct NewAgentBody: Encodable {
        var name: String; var description: String; var tools: String; var prompt: String
    }
    struct Created: Decodable { var path: String }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            Text("Create a subagent").font(K.F.title).foregroundStyle(K.C.text)

            VStack(alignment: .leading, spacing: 3) {
                Text("Name").font(K.F.micro).foregroundStyle(K.C.faint)
                TextField("test-runner", text: $name)
                    .textFieldStyle(.plain).font(K.F.code)
                    .padding(.horizontal, K.S.sm).padding(.vertical, 5)
                    .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                    .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
            }

            editor("When to use it",
                   "The only thing the main agent reads when deciding whether to delegate.",
                   $about, height: 60)
            editor("Its instructions", nil, $prompt, height: 100)

            if let error {
                Text(error).font(K.F.small).foregroundStyle(K.C.del)
            }

            HStack {
                Spacer()
                Button("Cancel") { done() }.buttonStyle(QuietButton())
                Button("Create") { create() }
                    .buttonStyle(SendButtonWide())
                    .disabled(name.isEmpty || about.isEmpty)
            }
        }
        .padding(K.S.xl).frame(width: 480).background(K.C.bg)
    }

    private func editor(
        _ label: String, _ note: String?, _ value: Binding<String>, height: CGFloat
    ) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(label).font(K.F.micro).foregroundStyle(K.C.faint)
            TextEditor(text: value)
                .font(K.F.body).scrollContentBackground(.hidden)
                .frame(height: height)
                .padding(.horizontal, K.S.xs).padding(.vertical, 3)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
                .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
            if let note {
                Text(note).font(K.F.micro).foregroundStyle(K.C.dim)
            }
        }
    }

    private func create() {
        Task {
            do {
                _ = try await client.post("/api/agents/create",
                                          body: NewAgentBody(name: name, description: about,
                                                             tools: "", prompt: prompt),
                                          as: Created.self)
                done()
            } catch { self.error = error.localizedDescription }
        }
    }
}
