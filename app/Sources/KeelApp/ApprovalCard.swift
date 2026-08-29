import SwiftUI

/// A command the agent is blocked on, asked where you are already looking.
///
/// Never a modal. The agent is genuinely stopped here — the `PreToolUse` hook holds the call open
/// for up to four minutes — but a sheet over the window would stop you too, and the whole design
/// is that nothing blocks the person.
///
/// Trust is first and largest because it is what someone working on their own repository actually
/// wants. Approving one program at a time is the toll that drives people to
/// `--dangerously-skip-permissions`, which turns permissions off everywhere and permanently.
struct ApprovalCard: View {
    let pending: Wire.Pending
    let model: SessionModel
    @State private var appeared = false

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "hand.raised.fill")
                    .font(.system(size: 10)).foregroundStyle(K.C.warn)
                Text("WAITING FOR YOU")
                    .font(.system(size: 9.5, weight: .semibold)).tracking(0.7)
                    .foregroundStyle(K.C.warn)
                Spacer()
                Text("the turn is paused").font(K.F.micro).foregroundStyle(K.C.faint)
            }

            Text(pending.command.isEmpty ? pending.tool : pending.command)
                .font(K.F.code)
                .foregroundStyle(K.C.text)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .padding(K.S.sm)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))

            VStack(alignment: .leading, spacing: K.S.xs) {
                Button {
                    model.answer(pending, allow: true, scope: "trust")
                } label: {
                    HStack {
                        VStack(alignment: .leading, spacing: 1) {
                            Text("Trust this project").font(K.F.small.weight(.semibold))
                            Text("stop asking here — withdrawable from the status bar")
                                .font(K.F.micro).foregroundStyle(K.C.faint)
                        }
                        Spacer()
                    }
                }
                .buttonStyle(PrimaryChoice())

                HStack(spacing: K.S.xs) {
                    Button("Allow \(pending.rules.joined(separator: " "))") {
                        model.answer(pending, allow: true, scope: "project")
                    }
                    .buttonStyle(QuietButton())
                    .help("Remembered in .keel/permissions.json for this project")

                    Button("Once") { model.answer(pending, allow: true, scope: "session") }
                        .buttonStyle(QuietButton())
                        .help("This conversation only, until Keel restarts")

                    Spacer()

                    Button("Deny") { model.answer(pending, allow: false, scope: "session") }
                        .buttonStyle(QuietButton(tone: K.C.del))
                        .help("The agent is told to stop rather than substitute another command")
                }
            }
        }
        .padding(K.S.md)
        .background(K.C.warn.opacity(0.07), in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(
            RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.warn.opacity(0.35), lineWidth: 1)
        )
        .scaleEffect(appeared ? 1 : 0.99)
        .opacity(appeared ? 1 : 0)
        .onAppear { withAnimation(K.M.settle) { appeared = true } }
    }
}

struct PrimaryChoice: ButtonStyle {
    @State private var hovering = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(K.C.text)
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: K.R.sm)
                    .fill(K.C.warn.opacity(configuration.isPressed ? 0.28 : (hovering ? 0.2 : 0.14)))
            )
            .overlay(
                RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.warn.opacity(0.4), lineWidth: 1)
            )
            .onHover { hovering = $0 }
    }
}
