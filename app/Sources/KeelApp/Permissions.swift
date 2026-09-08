import SwiftUI

/// What the agent may run here, and the one decision that removes the question.
///
/// Per-command approval on a repository you already own is not a safety property, it is a toll —
/// `docker`, then `wc`, then `grep` — and the way people pay a toll is by turning permissions off
/// everywhere and permanently. So there is one decision that removes the whole class: trust this
/// project. Scoped to one repository, stored outside it on this Mac, never the
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
    @State private var confirmingTrust = false

    struct View_: Decodable {
        var project: [String]
        var session: [String]
        var suggested: [String]
        var trusted: Bool
    }

    struct TrustBody: Encodable { var trusted: Bool }
    struct RuleBody: Encodable { var rule: String; var scope: String; var session: String? }

    var body: some SwiftUI.View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSection(
                "Trust",
                note: view?.trusted == true
                    ? "The agent runs commands here without asking. Withdrawing keeps the rules "
                      + "already approved."
                    : "Removes the per-command question for this repository on this Mac only. "
                      + "Old repository permission files are ignored; approve trust here again if needed."
            ) {
                // Granting confirms; withdrawing does not. Turning trust on is the one switch
                // in the app that changes what runs on this machine unattended.
                SettingsToggle("Trust this project", isOn: Binding(
                    get: { view?.trusted ?? false },
                    set: { on in if on { confirmingTrust = true } else { setTrust(false) } }))
                .alert("Trust this project?", isPresented: $confirmingTrust) {
                    Button("Trust") { setTrust(true) }
                    Button("Cancel", role: .cancel) {}
                } message: {
                    Text(TrustAlert.blurb)
                }
            }

            if let v = view, !v.project.isEmpty {
                SettingsSection("Allowed in this project") {
                    ForEach(v.project, id: \.self) { rule in
                        SettingsRow(title: rule, code: true) {
                            Button("Remove") { remove(rule, scope: "project") }
                                .buttonStyle(QuietButton())
                        }
                    }
                }
            }

            if let v = view, !v.session.isEmpty {
                SettingsSection(
                    "Allowed once, this conversation",
                    note: "These belong to this window's session and go when Keel does. Another "
                        + "window's agent is still asked."
                ) {
                    ForEach(v.session, id: \.self) { rule in
                        SettingsRow(title: rule, code: true) { EmptyView() }
                    }
                }
            }

            if let v = view, !v.suggested.isEmpty {
                SettingsSection(
                    "Suggested permissions",
                    note: "Not enabled automatically. Build scripts and installers can execute "
                        + "code. Review a rule before allowing it on this Mac."
                ) {
                    ForEach(v.suggested, id: \.self) { rule in
                        if !v.project.contains(rule) {
                            SettingsRow(title: rule, code: true) {
                                Button("Allow") { add(rule) }
                                    .buttonStyle(QuietButton())
                            }
                        }
                    }
                }
            }

            // A malformed rule that matches nothing is worse than a refusal: it looks approved and
            // keeps failing, so the daemon refuses anything not shaped like a tool pattern.
            SettingsSection(
                "Add a rule",
                note: "Claude Code's own pattern syntax, e.g. `Bash(docker *)`. The space before "
                    + "the star matters — without it, `Bash(git diff*)` also matches "
                    + "`git diff-index`."
            ) {
                HStack(spacing: K.S.sm) {
                    TextField("Bash(docker *)", text: $newRule)
                        .field()
                        .font(K.F.code)
                        .onSubmit { add() }
                    Button("Allow") { add() }
                        .buttonStyle(FilledButton())
                        .disabled(newRule.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }

            if let error {
                ErrorRow(message: error).padding(.top, K.S.md)
            }
        }
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

    private func add(_ candidate: String? = nil) {
        let rule = (candidate ?? newRule).trimmingCharacters(in: .whitespaces)
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
