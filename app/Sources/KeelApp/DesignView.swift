import SwiftUI

/// The pixel column. A UI change gets a before and an after beside its code diff, and a verdict
/// about whether anything actually moved.
struct DesignStrip: View {
    let design: Turn.Design

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            HStack(spacing: K.S.sm) {
                verdictLabel
                Text(design.selector)
                    .font(K.F.mono(10)).foregroundStyle(K.C.faint)
                    .lineLimit(1).truncationMode(.head)
                Spacer()
            }

            HStack(alignment: .top, spacing: K.S.md) {
                shot("before", design.before)
                Image(systemName: "arrow.right")
                    .font(.system(size: 10)).foregroundStyle(K.C.faint)
                    .padding(.top, 26)
                shot("after", design.after)
                Spacer()
            }

            if design.duplicated {
                HStack(alignment: .top, spacing: K.S.sm) {
                    Image(systemName: "doc.on.doc").font(.system(size: 10))
                    Text("A new component appeared and the likely source was never touched. Check "
                         + "it was edited rather than copied — that is how design drift starts.")
                        .fixedSize(horizontal: false, vertical: true)
                }
                .font(K.F.small).foregroundStyle(K.C.warn)
            }
        }
        .padding(K.S.md)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(K.C.surface, in: RoundedRectangle(cornerRadius: K.R.md))
        .overlay(RoundedRectangle(cornerRadius: K.R.md).stroke(K.C.line, lineWidth: 1))
    }

    @ViewBuilder
    private var verdictLabel: some View {
        switch design.verdict {
        case .changed:
            Pill(text: "PIXELS CHANGED", tone: .good)
        case .nothingChanged:
            // The failure the whole feature exists to catch: a green diff that changed nothing.
            HStack(spacing: 5) {
                Pill(text: "NOTHING CHANGED", tone: .warn)
                Text("the edit probably went to the wrong file")
                    .font(K.F.micro).foregroundStyle(K.C.warn)
            }
        case .unstable:
            HStack(spacing: 5) {
                Pill(text: "NOT COMPARED", tone: .neutral)
                Text("the page was still moving").font(K.F.micro).foregroundStyle(K.C.faint)
            }
        }
    }

    @ViewBuilder
    private func shot(_ label: String, _ image: NSImage?) -> some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            Text(label.uppercased())
                .font(.system(size: 8.5, weight: .semibold)).tracking(0.6)
                .foregroundStyle(K.C.faint)
            if let image {
                Image(nsImage: image)
                    .resizable().scaledToFit()
                    .frame(maxWidth: 220, maxHeight: 150)
                    .background(K.C.raised)
                    .clipShape(RoundedRectangle(cornerRadius: K.R.sm))
                    .overlay(
                        RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1)
                    )
            } else {
                RoundedRectangle(cornerRadius: K.R.sm).fill(K.C.well)
                    .frame(width: 110, height: 60)
                    .overlay(Text("none").font(K.F.micro).foregroundStyle(K.C.faint))
            }
        }
    }
    }

/// The staged pick, above the composer: what you clicked, and where Keel thinks it came from.
///
/// The candidates are on screen *before* the agent runs. Everyone else resolves this silently and
/// hopes; showing the ranking is the difference between a guess you can correct and a guess you
/// find out about from a diff that touched the wrong file.
struct PickedCard: View {
    @Bindable var model: SessionModel

    var body: some View {
        if let p = model.picked {
            VStack(alignment: .leading, spacing: K.S.xs) {
                HStack(spacing: K.S.sm) {
                    Image(systemName: "cursorarrow.rays")
                        .font(.system(size: 9)).foregroundStyle(K.C.accent)
                    Text("PICKED").font(.system(size: 9, weight: .semibold)).tracking(0.6)
                        .foregroundStyle(K.C.accent)
                    Text(p.selector).font(K.F.mono(10)).foregroundStyle(K.C.dim)
                        .lineLimit(1).truncationMode(.head)
                    Spacer()
                    CloseButton(size: 8) { model.picked = nil; model.pickedBefore = nil }
                }

                if p.hints.isEmpty {
                    Text("No source hint. Say what the element is if you know — otherwise the "
                         + "agent has to find it, and that is where these go wrong.")
                        .font(K.F.micro).foregroundStyle(K.C.faint)
                        .fixedSize(horizontal: false, vertical: true)
                } else {
                    // Ranked, and on screen before the agent runs. A silent guess at the wrong
                    // file is the failure this whole feature exists to catch.
                    ForEach(Array(p.hints.enumerated()), id: \.element.id) { i, h in
                        HStack(spacing: K.S.sm) {
                            Text(i == 0 ? "best" : h.kind)
                                .font(.system(size: 8.5, weight: i == 0 ? .semibold : .regular))
                                .foregroundStyle(i == 0 ? K.C.accent : K.C.faint)
                                .frame(width: 46, alignment: .leading)
                            Text(h.value).font(K.F.mono(10)).foregroundStyle(K.C.dim)
                                .lineLimit(1).truncationMode(.head)
                        }
                    }
                }
            }
            .padding(K.S.sm)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(K.C.accent.opacity(0.08), in: RoundedRectangle(cornerRadius: K.R.sm))
            .overlay(
                RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.accent.opacity(0.3), lineWidth: 1)
            )
        }
    }
}
