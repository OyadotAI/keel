import SwiftUI

/// What the agent may run here, and the one decision that removes the question.
///
/// Per-command approval on a repository you already own is not a safety property, it is a toll —
/// `docker`, then `wc`, then `grep` — and the way people pay a toll is by turning permissions off
/// everywhere and permanently. So there is one decision that removes the whole class: trust this
/// project. Scoped to one repository, stored in its own `.keel/permissions.json`, never the
/// default, and **withdrawable**, keeping the rules already approved.
///
/// Visible while it holds, which is the point of showing it here at all: a permission granted once
/// and then forgotten is the one that surprises you later.
struct PermissionsSettings: View {
    let client: Client
    let sessionId: String?

    @State private var view: View_?
    @State private var error: String?
    @State private var newRule = ""

    struct View_: Decodable {
        var project: [String]
        var session: [String]
        var suggested: [String]
        var trusted: Bool
    }

    struct TrustBody: Encodable { var trusted: Bool }
    struct RuleBody: Encodable { var rule: String; var scope: String; var session: String? }

    var body: some SwiftUI.View {
        Form {
            Section {
                Toggle("Trust this project", isOn: Binding(
                    get: { view?.trusted ?? false },
                    set: { setTrust($0) }))
                .toggleStyle(.switch)
            } footer: {
                Text(view?.trusted == true
                     ? "The agent runs commands here without asking. Withdrawing keeps the rules "
                       + "already approved."
                     : "Removes the per-command question for this repository only. It is never on "
                       + "by default, and it can be withdrawn from here at any time.")
                    .font(.system(size: 11)).foregroundStyle(.secondary)
            }

            if let v = view, !v.project.isEmpty {
                Section("Allowed in this project") {
                    ForEach(v.project, id: \.self) { rule in
                        HStack {
                            Text(rule).font(.system(size: 11, design: .monospaced))
                            Spacer()
                            Button("Remove") { remove(rule, scope: "project") }
                                .controlSize(.small)
                        }
                    }
                }
            }

            if let v = view, !v.session.isEmpty {
                Section {
                    ForEach(v.session, id: \.self) { rule in
                        Text(rule).font(.system(size: 11, design: .monospaced))
                    }
                } header: {
                    Text("Allowed once, this conversation")
                } footer: {
                    Text("These belong to this window's session and go when Keel does. Another "
                         + "window's agent is still asked.")
                        .font(.system(size: 11)).foregroundStyle(.secondary)
                }
            }

            if let v = view, !v.suggested.isEmpty {
                Section {
                    ForEach(v.suggested, id: \.self) { rule in
                        Text(rule).font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(.secondary)
                    }
                } header: {
                    Text("The project's own commands")
                } footer: {
                    // Applied, not suggested — shown so it is visible *why* the agent can run
                    // `make` without ever having asked.
                    Text("Applied automatically. Running what a repository declares about itself "
                         + "is the reason the agent is here.")
                        .font(.system(size: 11)).foregroundStyle(.secondary)
                }
            }

            Section {
                HStack {
                    TextField("Bash(docker *)", text: $newRule)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(size: 11, design: .monospaced))
                        .onSubmit { add() }
                    Button("Allow") { add() }
                        .controlSize(.small)
                        .disabled(newRule.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            } header: {
                Text("Add a rule")
            } footer: {
                // A malformed rule that matches nothing is worse than a refusal: it looks approved
                // and keeps failing, so the daemon refuses anything not shaped like a tool pattern.
                Text("Claude Code's own pattern syntax, e.g. `Bash(docker *)`. The space before the "
                     + "star matters — without it, `Bash(git diff*)` also matches `git diff-index`.")
                    .font(.system(size: 11)).foregroundStyle(.secondary)
            }

            if let error {
                Text(error).font(.system(size: 11)).foregroundStyle(.red)
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .frame(maxWidth: .infinity, alignment: .leading)
        .task { await refresh() }
    }

    private func refresh() async {
        var q: [String: String] = [:]
        if let sessionId { q["session"] = sessionId }
        view = try? await client.get("/api/permissions", q)
    }

    private func setTrust(_ on: Bool) {
        Task {
            do {
                _ = try await client.post("/api/permissions/trust", body: TrustBody(trusted: on),
                                          as: Bool.self)
                await refresh()
            } catch { self.error = error.localizedDescription }
        }
    }

    private func add() {
        let rule = newRule.trimmingCharacters(in: .whitespaces)
        guard !rule.isEmpty else { return }
        Task {
            do {
                _ = try await client.post("/api/permissions/add",
                                          body: RuleBody(rule: rule, scope: "project",
                                                         session: sessionId),
                                          as: Bool.self)
                newRule = ""
                error = nil
                await refresh()
            } catch { self.error = error.localizedDescription }
        }
    }

    private func remove(_ rule: String, scope: String) {
        Task {
            do {
                _ = try await client.post("/api/permissions/remove",
                                          body: RuleBody(rule: rule, scope: scope, session: sessionId),
                                          as: Bool.self)
                await refresh()
            } catch { self.error = error.localizedDescription }
        }
    }
}
