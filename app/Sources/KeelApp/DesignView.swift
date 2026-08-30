import SwiftUI

/// The pixel column. A UI change gets a before and an after beside its code diff, and a verdict
/// about whether anything actually moved.
struct DesignStrip: View {
    let design: Turn.Design

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            // One row per pin. Several pins used to capture several before-images and compare
            // exactly one of them, which read as a verdict about all of them.
            ForEach(design.pins) { pin in
                VStack(alignment: .leading, spacing: K.S.sm) {
                    HStack(spacing: K.S.sm) {
                        verdictLabel(pin.verdict)
                        Text(pin.selector)
                            .font(K.F.codeTiny).foregroundStyle(K.C.faint)
                            .lineLimit(1).truncationMode(.head)
                        Spacer()
                    }
                    HStack(alignment: .top, spacing: K.S.md) {
                        shot("before", pin.before)
                        Image(systemName: "arrow.right")
                            .font(K.F.tiny).foregroundStyle(K.C.faint)
                            .padding(.top, K.S.xl)
                        shot("after", pin.after)
                        Spacer()
                    }
                }
            }

            // What the page itself said moved — with or without a pin. This is the answer to
            // "what did that do to the site" for a turn nobody pointed at anything for.
            if !design.regions.isEmpty {
                HStack(alignment: .top, spacing: K.S.md) {
                    VStack(alignment: .leading, spacing: K.S.xs) {
                        Text("CHANGED ON SCREEN")
                            .sectionLabel()
                            .foregroundStyle(K.C.accent)
                        ForEach(Array(design.regions.prefix(6).enumerated()), id: \.offset) { i, r in
                            HStack(spacing: K.S.xs) {
                                Text("\(i + 1)").font(K.F.codeTiny.weight(.semibold)).foregroundStyle(K.C.accent)
                                    .frame(width: 14)
                                Text(r.tag).font(K.F.codeTiny).foregroundStyle(K.C.dim)
                                Text(r.text.isEmpty ? r.selector : r.text)
                                    .font(K.F.micro).foregroundStyle(K.C.faint)
                                    .lineLimit(1).truncationMode(.tail)
                            }
                        }
                    }
                    if let shot = design.pageAfter {
                        Image(nsImage: shot)
                            .resizable().scaledToFit()
                            .frame(maxWidth: 220, maxHeight: 150)
                            .background(K.C.raised)
                            .clipShape(RoundedRectangle(cornerRadius: K.R.sm))
                            .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
                    }
                    Spacer()
                }
            }

            if design.duplicated {
                HStack(alignment: .top, spacing: K.S.sm) {
                    Image(systemName: "doc.on.doc").font(K.F.tiny)
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
    private func verdictLabel(_ verdict: DesignCheck.Verdict) -> some View {
        switch verdict {
        case .changed:
            Pill(text: "PIXELS CHANGED", tone: .good)
        case .nothingChanged:
            // The failure the whole feature exists to catch: a green diff that changed nothing.
            HStack(spacing: K.S.snug) {
                Pill(text: "NOTHING CHANGED", tone: .warn)
                Text("the edit probably went to the wrong file")
                    .font(K.F.micro).foregroundStyle(K.C.warn)
            }
        case .notCompared(let why):
            // The reason, not a guess at one. "The page was still moving" was printed for a
            // closed pane, a scrolled-away element and a deleted one alike.
            HStack(spacing: K.S.snug) {
                Pill(text: "NOT COMPARED", tone: .neutral)
                Text(why).font(K.F.micro).foregroundStyle(K.C.faint)
                    .lineLimit(2).fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder
    private func shot(_ label: String, _ image: NSImage?) -> some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            Text(label.uppercased())
                .sectionLabel()
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

/// The pins, above the composer: each element you pointed at, with a note.
///
/// The candidates are on screen *before* the agent runs. Everyone else resolves this silently and
/// hopes; showing the ranking is the difference between a guess you can correct and a guess you
/// find out about from a diff that touched the wrong file.
struct PinList: View {
    @Bindable var model: SessionModel

    var body: some View {
        if !model.pins.isEmpty {
            VStack(alignment: .leading, spacing: K.S.xs) {
                ForEach(Array(model.pins.enumerated()), id: \.element.id) { i, pin in
                    PinRow(model: model, index: i, pin: pin)
                }
            }
        }
    }
}

private struct PinRow: View {
    @Bindable var model: SessionModel
    let index: Int
    let pin: SessionModel.Pin
    @State private var showHints = false
    @State private var showProps = false

    private var note: Binding<String> {
        Binding(get: { model.pins.first { $0.id == pin.id }?.note ?? "" },
                set: { v in if let i = model.pins.firstIndex(where: { $0.id == pin.id }) { model.pins[i].note = v } })
    }

    /// The properties worth reading at a glance, in the order a designer asks about them.
    private static let order = ["font-size", "font-weight", "line-height", "letter-spacing",
        "color", "background-color", "padding", "margin", "gap", "border", "border-radius",
        "display", "flex-direction", "justify-content", "align-items", "opacity", "box-shadow"]

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            HStack(spacing: K.S.sm) {
                Text("\(index + 1)")
                    .font(K.F.tiny.weight(.bold)).foregroundStyle(.white)
                    .frame(width: 18, height: 18).background(K.C.accent, in: Circle())
                Text(pin.picked.text.isEmpty ? pin.picked.selector : "\(pin.picked.tag) · \(pin.picked.text)")
                    .font(K.F.codeTiny).foregroundStyle(K.C.dim)
                    .lineLimit(1).truncationMode(.tail)
                Spacer()
                if !pin.picked.style.isEmpty {
                    Button(showProps ? "hide style" : "style") { showProps.toggle() }
                        .buttonStyle(.plain).font(K.F.micro).foregroundStyle(K.C.faint)
                }
                if !pin.picked.hints.isEmpty {
                    Button(showHints ? "hide source" : "source") { showHints.toggle() }
                        .buttonStyle(.plain).font(K.F.micro).foregroundStyle(K.C.faint)
                }
                CloseButton(size: 10, label: "Remove pin \(index + 1)") { model.removePin(pin.id) }
            }

            // What the selection *is* — size, type, colour. It was collected for the agent and
            // never shown to the person deciding what to ask for.
            HStack(spacing: K.S.xs) {
                Text(pin.picked.summary)
                    .font(K.F.codeTiny).foregroundStyle(K.C.faint)
                    .lineLimit(1).truncationMode(.tail)
                if pin.picked.unique == false {
                    Text("matches several")
                        .font(K.F.micro).foregroundStyle(K.C.warn)
                        .hint("This selector is not unique, so the after-photo may be of a "
                              + "different element with the same shape.")
                }
            }

            // What was dragged, in the words it will be asked for.
            ForEach(Array(pin.nudges.enumerated()), id: \.offset) { i, n in
                HStack(spacing: K.S.xs) {
                    Image(systemName: "hand.draw").font(K.F.micro).foregroundStyle(K.C.accent)
                    Text(n).font(K.F.codeTiny).foregroundStyle(K.C.text)
                        .lineLimit(2).fixedSize(horizontal: false, vertical: true)
                    Spacer()
                    CloseButton(size: 9, label: "Undo this change") {
                        if let p = model.pins.firstIndex(where: { $0.id == pin.id }) {
                            model.pins[p].nudges.remove(at: i)
                        }
                    }
                }
            }

            TextField(pin.nudges.isEmpty ? "What should change here?" : "Anything else?", text: note)
                .field().font(K.F.small)

            if showProps {
                let props = Self.order.compactMap { key in
                    pin.picked.style[key].map { (key, $0) }
                }
                ForEach(props, id: \.0) { key, value in
                    HStack(spacing: K.S.sm) {
                        Text(key).font(K.F.codeTiny).foregroundStyle(K.C.faint)
                            .frame(width: 108, alignment: .leading)
                        if key.hasSuffix("color") {
                            RoundedRectangle(cornerRadius: 2)
                                .fill(Color(css: value) ?? .clear)
                                .frame(width: 10, height: 10)
                                .overlay(RoundedRectangle(cornerRadius: 2).stroke(K.C.line, lineWidth: 1))
                        }
                        Text(value).font(K.F.codeTiny).foregroundStyle(K.C.dim)
                            .lineLimit(1).truncationMode(.tail)
                    }
                }
            }

            if showHints {
                // Ranked, and on screen before the agent runs. A silent guess at the wrong file
                // is the failure this whole feature exists to catch.
                ForEach(Array(pin.picked.hints.enumerated()), id: \.element.id) { i, h in
                    HStack(spacing: K.S.sm) {
                        Text(i == 0 ? "best" : h.kind)
                            .font(K.F.tiny.weight(i == 0 ? .semibold : .regular))
                            .foregroundStyle(i == 0 ? K.C.accent : K.C.faint)
                            .frame(width: 46, alignment: .leading)
                        Text(h.value).font(K.F.codeTiny).foregroundStyle(K.C.dim)
                            .lineLimit(1).truncationMode(.head)
                    }
                }
            }
        }
        .padding(K.S.sm)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(K.C.accent.wash, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.accent.opacity(0.3), lineWidth: 1))
    }
}

extension Color {
    /// A CSS colour as the page computed it — always `rgb()` or `rgba()` from `getComputedStyle`.
    init?(css: String) {
        let n = css.split(whereSeparator: { !$0.isNumber && $0 != "." }).compactMap { Double($0) }
        guard n.count >= 3 else { return nil }
        self.init(.sRGB, red: n[0] / 255, green: n[1] / 255, blue: n[2] / 255,
                  opacity: n.count > 3 ? n[3] : 1)
    }
}
