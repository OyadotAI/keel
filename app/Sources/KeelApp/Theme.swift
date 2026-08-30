import AppKit
import SwiftUI

/// The design system, in one place.
///
/// Every number here is a decision, and the reason they are tokens rather than literals scattered
/// through views is that an IDE is judged on consistency: two panels with 11px and 11.5px text
/// look broken in a way nobody can name. Values come in scales, and views pick from the scale.
enum K {

    // MARK: - Colour
    //
    // Built from a neutral ramp rather than from named greys, so light and dark are the same
    // design with the ramp reversed. Surfaces are near-black rather than black: pure black kills
    // the elevation cues that tell you which pane is in front.

    /// Built from a neutral ramp rather than named greys, so light and dark are the same design
    /// with the ramp reversed.
    ///
    /// Declared in code rather than in an asset catalog. SwiftPM copies `.xcassets` into the
    /// bundle without compiling it, so only some names resolved at runtime and the rest rendered
    /// as nothing — a whole-app visual failure with no error anywhere. A `dynamicProvider` needs
    /// no build step, and puts the two values on the same line where a change to one is visibly a
    /// change to the other.
    enum C {
        /// The window's own ground.
        static let bg = pair(0xFBFBFA, 0x131417)
        /// A pane on the ground: rails, sidebars, the status bar.
        static let surface = pair(0xF3F4F6, 0x191B20)
        /// Navigation selection and persistent controls.
        static let chrome = pair(0xE9EBEF, 0x22252B)
        /// A card on a pane: a diff, an approval, a picked element.
        static let raised = pair(0xFFFFFF, 0x1E2126)
        /// Inputs and wells — recessed rather than raised.
        static let well = pair(0xEFEFED, 0x101215)

        static let line = pair(0xE3E3E0, 0x25282E)
        static let lineStrong = pair(0xCFCFCB, 0x343841)

        /// Primary reading text.
        static let text = pair(0x1B1C1E, 0xE6E8EC)
        /// Secondary: labels and metadata still meant to be read.
        static let dim = pair(0x5C5F66, 0x9BA1AC)
        /// Tertiary: present, not competing. Line numbers, timestamps, counts.
        static let faint = pair(0x8B8F98, 0x6B7280)

        static let accent = pair(0x2563EB, 0x5B9DFF)
        static let add = pair(0x177245, 0x4ADE80)
        static let del = pair(0xB3261E, 0xF87171)
        static let warn = pair(0xB45309, 0xFBBF24)

        /// Diff row grounds. Far weaker than the glyph colours on purpose: a diff is read for its
        /// content, and a saturated background makes forty changed lines unreadable.
        static let addBG = pair(0xE7F6EC, 0x12291B)
        static let delBG = pair(0xFCEBEA, 0x2B1517)

        /// The three neutral fills every hand-drawn control reaches for: a surface that is barely
        /// there, the same one under the pointer, and a selected one. Written as 24 distinct
        /// `.opacity()` literals before they had names — `0.03`, `0.04`, `0.05`, `0.055`, `0.06`,
        /// `0.07`, `0.08` were all one intent, and two controls side by side disagreed by a
        /// hundredth, which is a difference you can see and cannot name.
        static let ghost = text.opacity(0.04)
        static let hover = text.opacity(0.08)
        static let tint = accent.opacity(0.16)

        /// Every token, for the test that checks contrast and that both appearances differ.
        static let all: [(String, Color)] = [
            ("bg", bg), ("surface", surface), ("chrome", chrome), ("raised", raised), ("well", well),
            ("line", line), ("lineStrong", lineStrong),
            ("text", text), ("dim", dim), ("faint", faint),
            ("accent", accent), ("add", add), ("del", del), ("warn", warn),
            ("addBG", addBG), ("delBG", delBG),
        ]

        /// One token, two values, resolved by the appearance in effect when it is drawn.
        static func pair(_ light: UInt32, _ dark: UInt32) -> Color {
            Color(nsColor: NSColor(name: nil) { appearance in
                let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
                return hex(isDark ? dark : light)
            })
        }

        static func hex(_ v: UInt32) -> NSColor {
            NSColor(srgbRed: Double((v >> 16) & 0xFF) / 255,
                    green: Double((v >> 8) & 0xFF) / 255,
                    blue: Double(v & 0xFF) / 255,
                    alpha: 1)
        }
    }

    // MARK: - Type
    //
    // Two families and one scale. Mono for anything that is code, a path, a command or a figure —
    // including numbers in prose, so a cost that ticks up does not reflow the line around it.

    enum F {
        static func ui(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
            .system(size: size, weight: weight)
        }
        static func mono(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
            .system(size: size, weight: weight, design: .monospaced)
        }

        /// 10 · the floor, and the size of an eyebrow or a badge. Named because it was already
        /// the most-used size in the app by a distance — 115 of the 147 inline `.system(size:)`
        /// calls were this one, written out because the scale had no word for it.
        static let tiny = ui(10)
        /// 11 · a label, a count, a timestamp. The platform's small-system size and the floor:
        /// nothing meant to be read is set below `floor`, and a test says so.
        static let micro = ui(11)
        static let floor: CGFloat = 10
        /// Every named size, for the test.
        static let sizes: [(String, CGFloat)] = [
            ("tiny", 10), ("micro", 11), ("small", 12), ("body", 13), ("reading", 14),
            ("title", 16), ("display", 24),
            ("codeTiny", 10), ("codeSmall", 11), ("code", 12),
        ]
        /// 12 · secondary rows
        static let small = ui(12)
        /// 13 · body, the default
        static let body = ui(13)
        /// 16 · a section that has to be found
        static let title = ui(16, .semibold)
        /// 24 · the one heading on a screen
        static let display = ui(24, .semibold)

        /// 14 · text you type into or read at length: the composer, the palette's query, and the
        /// assistant's prose. A step above `body` because a paragraph and a table row are not read
        /// the same way.
        static let reading = ui(14)

        static let code = mono(12)
        static let codeSmall = mono(11)
        /// 10 · a figure in a row — a count, a token total, a duration. Mono so a number that
        /// ticks up does not reflow the line around it. This was the single most-written size in
        /// the app, at 56 call sites, and the scale had no word for it.
        static let codeTiny = mono(10)
    }

    // MARK: - Space
    //
    // A 4pt grid with two half-steps. Rows in a dense rail genuinely want 2 and 6; pretending
    // they did not meant forty literal `spacing: 6`s that the tokens could not see.

    enum S {
        /// The tight end, named rather than pretended away. A 4pt grid cannot express the inset a
        /// dense row actually wants, so `1`, `3` and `5` were written as literals ~120 times —
        /// including inside `HoverRow`, `FilledButton`, `QuietButton` and `Pill`, the components
        /// that define the grid. A scale the system itself does not follow is not a scale.
        static let hair: CGFloat = 1
        static let tight: CGFloat = 3
        static let snug: CGFloat = 5
        static let xxs: CGFloat = 2
        static let xs: CGFloat = 4
        static let half: CGFloat = 6
        static let sm: CGFloat = 8
        static let md: CGFloat = 12
        static let lg: CGFloat = 16
        static let xl: CGFloat = 24
        static let xxl: CGFloat = 40
    }

    /// The letter-spacing an uppercase label gets. One value: the app had 0.7, 0.6, 0.4 and 0.3
    /// for the same treatment, which is a difference nobody can name and everybody can see.
    static let tracking: CGFloat = 0.6

    enum R {
        static let sm: CGFloat = 4
        static let md: CGFloat = 6
        static let lg: CGFloat = 10
    }

    /// Motion that carries meaning and nothing else. A diff row arriving is information; a panel
    /// easing open is decoration, and decoration in a tool you use all day becomes latency.
    enum M {
        static let quick = Animation.easeOut(duration: 0.12)
        static let settle = Animation.easeOut(duration: 0.18)
        /// A restrained spring for live evidence entering and sections changing size.
        // Panels should feel responsive and calm; spring overshoot makes dense rows jump.
        static let flow = Animation.easeInOut(duration: 0.22)
    }
}

// MARK: - Shared chrome

/// A quiet section header in a rail. Sentence case reads like navigation rather than telemetry.
struct RailHeader: View {
    let title: String
    var trailing: String?

    init(_ title: String, trailing: String? = nil) {
        self.title = title
        self.trailing = trailing
    }

    var body: some View {
        HStack(spacing: K.S.xs) {
            Text(title)
                .font(K.F.small.weight(.medium))
            Spacer()
            if let trailing {
                Text(trailing).font(K.F.mono(10)).monospacedDigit()
            }
        }
        .foregroundStyle(K.C.dim)
        .padding(.horizontal, K.S.md)
        .padding(.top, K.S.lg)
        .padding(.bottom, K.S.sm)
    }
}

/// The heading rhythm used by primary content surfaces.
struct ContentHeading: View {
    let title: String
    var detail: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            Text(title).font(K.F.title).foregroundStyle(K.C.text)
            if let detail {
                Text(detail).font(K.F.small).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A content section with one consistent title, inset and separator treatment.
struct ContentSection<Content: View>: View {
    let title: String
    var detail: String? = nil
    @ViewBuilder let content: Content

    init(_ title: String, detail: String? = nil, @ViewBuilder content: () -> Content) {
        self.title = title
        self.detail = detail
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.md) {
            ContentHeading(title: title, detail: detail)
            content
        }
        .padding(.vertical, K.S.lg)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .bottom) { Hairline() }
    }
}

/// A row that highlights on hover, which is how a dense list stays navigable without borders.
///
/// A `Button`, not a tap gesture: a gesture on a plain view is invisible to the keyboard and to
/// VoiceOver, and every list in the side panel is built from this row. Same pixels, focusable.
struct HoverRow<Content: View>: View {
    var selected = false
    // Content before action so `HoverRow { … } action: { … }` reads in the order it happens.
    @ViewBuilder var content: Content
    var action: (() -> Void)?
    @State private var hovering = false

    var body: some View {
        Button { action?() } label: {
            content
                .padding(.horizontal, K.S.md)
                .padding(.vertical, K.S.tight)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(selected ? K.C.tint : (hovering ? K.C.hover : .clear))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

extension View {
    /// The eyebrow above a section: 10pt, semibold, tracked.
    ///
    /// A modifier rather than a wrapping view, because the 22 places that wrote this out by hand
    /// each already set their own colour on the same `Text`, and a wrapper would have meant
    /// restructuring 22 call sites to change none of their pixels. Three tracking values —
    /// 0.7, 0.6 and 0.4 — collapse to `K.tracking`.
    func sectionLabel() -> some View {
        font(K.F.tiny.weight(.semibold)).tracking(K.tracking)
    }

    /// A tooltip that is also the VoiceOver label. Icon-only controls had thirty tooltips and no
    /// labels; the strings were there, they were just pointer-only.
    func hint(_ text: String) -> some View {
        help(text).accessibilityLabel(text)
    }
}

/// The one filled button. Every primary call to action in the app is this, so a filled accent
/// block means "the thing to do here" everywhere it appears.
///
/// It says "the one" because there were three. `SendButton` in `ChatRail` and `SendButtonWide` in
/// `Welcome` were byte-identical to each other but for a point of vertical padding, and each
/// carried a comment claiming to be the only primary style. What they had that this did not was a
/// disabled appearance, which is why they existed at all; it is here now.
struct FilledButton: ButtonStyle {
    var tone: Color = K.C.accent
    @Environment(\.isEnabled) private var enabled
    @State private var hovering = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(K.F.small.weight(.semibold))
            .foregroundStyle(enabled ? Color.white : K.C.faint)
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.snug)
            .background(
                RoundedRectangle(cornerRadius: K.R.sm).fill(
                    enabled
                        ? tone.opacity(configuration.isPressed ? 0.8 : (hovering ? 0.92 : 1))
                        : K.C.line
                )
            )
            .onHover { hovering = $0 }
    }
}

/// A button that reads as a control without shouting. Stock `.bordered` is too heavy for a dense
/// surface, and `.plain` gives no affordance at all.
///
/// The most-used control in the app — and it lived in `TurnStage.swift`, a feature file, where
/// nobody looking for the design system would find it. That is most of why the system has been
/// re-implemented as often as it has been used.
struct QuietButton: ButtonStyle {
    var tone: Color = K.C.dim
    @State private var hovering = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(K.F.small)
            .foregroundStyle(configuration.isPressed ? K.C.text : tone)
            .padding(.horizontal, K.S.sm).padding(.vertical, K.S.tight)
            .background(
                RoundedRectangle(cornerRadius: K.R.sm)
                    .fill(hovering ? K.C.hover : K.C.ghost)
            )
            .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
            .onHover { hovering = $0 }
    }
}

extension View {
    /// Make a hand-drawn row a real control. A tap gesture on a view is invisible to the keyboard
    /// and to VoiceOver; wrapping the same view in a plain `Button` changes nothing on screen.
    func asButton(_ action: @escaping () -> Void) -> some View {
        Button(action: action) { self }.buttonStyle(.plain)
    }
}

/// A small status pill. One shape for every state so they read as a set.

struct Pill: View {
    let text: String
    let tone: Tone
    enum Tone { case good, bad, warn, neutral, accent }

    var body: some View {
        Text(text)
            .font(K.F.tiny.weight(.semibold))
            .tracking(K.tracking)
            .padding(.horizontal, K.S.snug).padding(.vertical, K.S.xxs)
            .background(color.opacity(0.16), in: RoundedRectangle(cornerRadius: K.R.sm - 1))
            .foregroundStyle(color)
    }

    private var color: Color {
        switch tone {
        case .good: K.C.add
        case .bad: K.C.del
        case .warn: K.C.warn
        case .accent: K.C.accent
        case .neutral: K.C.faint
        }
    }
}

extension Color {
    /// A faint wash of this colour behind a row or a bar — a turn that is live, a hunk that is
    /// picked, the working bar. The tone carries the meaning; the strength is not a per-site
    /// decision, and it was made as one eight different ways.
    var wash: Color { opacity(0.08) }
}

/// A hairline. `Divider()` renders heavier than a real 1px rule at 2x.
struct Hairline: View {
    var body: some View {
        Rectangle().fill(K.C.line).frame(height: 1)
    }
}


/// A close button you can actually hit.
///
/// The first version was a 7pt glyph with no padding, so the hit target was about 7×7pt — well
/// under the ~20pt anything on a pointer surface needs, and it took several tries. The glyph stays
/// small because it is not the point of the row; the *target* around it does not.
struct CloseButton: View {
    var size: CGFloat = 9
    var label = "Close"
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: "xmark")
                .font(.system(size: size, weight: .bold))
                .foregroundStyle(hovering ? K.C.text : K.C.faint)
                .frame(width: 20, height: 20)
                .background(
                    Circle().fill(hovering ? K.C.text.opacity(0.12) : .clear)
                )
                // The background is only painted on hover, so without this the transparent half
                // of the frame does not take the click.
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .onHover { hovering = $0 }
    }
}


/// Items in a row that wraps to the next line.
///
/// Attachments were in a horizontal `ScrollView`, which meant a chip could be off-screen and the
/// scroll view took a click to settle before its buttons responded — the "click a few times"
/// problem. Two or three chips should simply be visible.
struct Flow: Layout {
    var spacing: CGFloat = K.S.half

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, rowHeight: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x > 0, x + size.width > width {
                x = 0
                y += rowHeight + spacing
                rowHeight = 0
            }
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
        return CGSize(width: proposal.width ?? x, height: y + rowHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize,
                       subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, rowHeight: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x > bounds.minX, x + size.width > bounds.maxX {
                x = bounds.minX
                y += rowHeight + spacing
                rowHeight = 0
            }
            view.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
    }
}


/// `12.4k`, `1.2M` — tokens are read at a glance or not at all.
func compact(_ n: Int) -> String {
    switch n {
    case ..<1000: return "\(n)"
    case ..<1_000_000: return String(format: "%.1fk", Double(n) / 1000)
    default: return String(format: "%.2fM", Double(n) / 1_000_000)
    }
}


/// A text field on the design system: a well with a hairline, like the composer.
///
/// The settings panes used `.roundedBorder`, which is AppKit's look, in a window where every
/// other input is a well — so two of the four tabs looked like a different application.
struct Field: ViewModifier {
    @FocusState private var focused: Bool
    func body(content: Content) -> some View {
        content
            .textFieldStyle(.plain)
            .focused($focused)
            .padding(.horizontal, K.S.sm).padding(.vertical, K.S.xs + 1)
            .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            .overlay(RoundedRectangle(cornerRadius: K.R.sm)
                .stroke(focused ? K.C.accent : K.C.line, lineWidth: 1))
    }
}

extension View {
    func field() -> some View { modifier(Field()) }
}
