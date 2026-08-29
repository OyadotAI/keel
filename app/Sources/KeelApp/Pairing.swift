import SwiftUI

/// Reaching this Keel from a phone.
///
/// The Mac half only. There is no iOS client yet, and the honest thing is that the wire protocol is
/// the deliverable here: a paired device is a bearer token against the same loopback API, so a
/// second Mac stands in for a phone perfectly while the phone is being written.
@MainActor
@Observable
final class PairingModel {
    let client: Client
    var code: String?
    var devices: [Device] = []
    var error: String?

    struct Device: Decodable, Identifiable {
        var id: String
        var name: String
        var issued: String
    }

    init(client: Client) { self.client = client }

    func refresh() async {
        devices = (try? await client.get("/api/pair/devices")) ?? []
    }

    struct Empty: Encodable {}
    struct CodeView: Decodable { var code: String; var expires_in: Int }

    func begin() async {
        do {
            let v: CodeView = try await client.post("/api/pair/begin", body: Empty())
            code = v.code
            error = nil
            // The code dies on its own after two minutes; the UI should stop showing it then, or
            // it is advertising something that no longer works.
            let seconds = v.expires_in
            Task {
                try? await Task.sleep(for: .seconds(seconds))
                if self.code == v.code { self.code = nil }
            }
        } catch {
            self.error = error.localizedDescription
        }
    }

    func revoke(_ id: String) async {
        var req = URLRequest(url: await client.base.appendingPathComponent("api/pair/devices/\(id)"))
        req.httpMethod = "DELETE"
        _ = try? await URLSession.shared.data(for: req)
        await refresh()
    }
}

struct PairingSettings: View {
    @State var model: PairingModel

    var body: some View {
        Form {
            Section {
                if let code = model.code {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(code)
                            .font(.system(size: 34, weight: .medium, design: .monospaced))
                            .tracking(6)
                            .monospacedDigit()
                        Text("Type this on the other device within two minutes.")
                            .font(.system(size: 11)).foregroundStyle(.secondary)
                    }
                } else {
                    Button("Pair a device") { Task { await model.begin() } }
                }
                if let e = model.error {
                    Text(e).font(.system(size: 11)).foregroundStyle(.red)
                }
            } header: {
                Text("Pairing")
            } footer: {
                // Said here rather than discovered later. A phone that goes quiet the moment it is
                // backgrounded is the single most-reported disappointment with this kind of feature
                // elsewhere, and it is a consequence of not running a relay — which is the trade
                // being made on purpose.
                Text("Pairing works while this Mac is awake and the device can reach it — on the "
                     + "same network, or anywhere over Tailscale. There are no push notifications "
                     + "to a backgrounded phone: that needs a relay server, and Keel does not have "
                     + "one so that your code never leaves your machine.")
                    .font(.system(size: 11)).foregroundStyle(.secondary)
            }

            Section("Paired devices") {
                if model.devices.isEmpty {
                    Text("None. Keel listens only on this machine until a device is paired.")
                        .font(.system(size: 11)).foregroundStyle(.secondary)
                }
                ForEach(model.devices) { d in
                    HStack {
                        Text(d.name)
                        Spacer()
                        Button("Revoke") { Task { await model.revoke(d.id) } }
                            .controlSize(.small)
                    }
                }
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .frame(maxWidth: .infinity, alignment: .leading)
        .task { await model.refresh() }
    }
}
