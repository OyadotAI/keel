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
        static let surface = pair(0xF4F4F2, 0x181A1E)
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

        /// Every token, for the test that checks contrast and that both appearances differ.
        static let all: [(String, Color)] = [
            ("bg", bg), ("surface", surface), ("raised", raised), ("well", well),
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

        /// 11 · a label, a count, a timestamp. The platform's small-system size and the floor:
        /// nothing meant to be read is set below `floor`, and a test says so.
        static let micro = ui(11)
        static let floor: CGFloat = 10
        /// Every named size, for the test.
        static let sizes: [(String, CGFloat)] = [
            ("micro", 11), ("small", 11.5), ("body", 12.5), ("title", 15), ("display", 22),
            ("code", 11.5), ("codeSmall", 11),
        ]
        /// 11.5 · secondary rows
        static let small = ui(11.5)
        /// 12.5 · body, the default
        static let body = ui(12.5)
        /// 15 · a section that has to be found
        static let title = ui(15, .semibold)
        /// 22 · the one heading on a screen
        static let display = ui(22, .semibold)

        static let code = mono(11.5)
        static let codeSmall = mono(11)
    }

    // MARK: - Space
    //
    // A 4pt grid with two half-steps. Rows in a dense rail genuinely want 2 and 6; pretending
    // they did not meant forty literal `spacing: 6`s that the tokens could not see.

    enum S {
        static let xxs: CGFloat = 2
        static let xs: CGFloat = 4
        static let half: CGFloat = 6
        static let sm: CGFloat = 8
        static let md: CGFloat = 12
        static let lg: CGFloat = 16
        static let xl: CGFloat = 24
        static let xxl: CGFloat = 40
    }

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
    }
}

// MARK: - Shared chrome

/// A section header in a rail. Uppercase, tracked, faint — present without competing.
struct RailHeader: View {
    let title: String
    var trailing: String?

    init(_ title: String, trailing: String? = nil) {
        self.title = title
        self.trailing = trailing
    }

    var body: some View {
        HStack(spacing: K.S.xs) {
            Text(title.uppercased())
                .font(.system(size: 10, weight: .semibold))
                .tracking(0.7)
            Spacer()
            if let trailing {
                Text(trailing).font(K.F.mono(10)).monospacedDigit()
            }
        }
        .foregroundStyle(K.C.faint)
        .padding(.horizontal, K.S.md)
        .padding(.top, K.S.md)
        .padding(.bottom, K.S.xs)
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
                .padding(.vertical, 3)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    selected ? K.C.accent.opacity(0.16)
                             : (hovering ? K.C.text.opacity(0.06) : .clear)
                )
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

extension View {
    /// A tooltip that is also the VoiceOver label. Icon-only controls had thirty tooltips and no
    /// labels; the strings were there, they were just pointer-only.
    func hint(_ text: String) -> some View {
        help(text).accessibilityLabel(text)
    }
}

/// A small status pill. One shape for every state so they read as a set.
struct Pill: View {
    let text: String
    let tone: Tone
    enum Tone { case good, bad, warn, neutral, accent }

    var body: some View {
        Text(text)
            .font(.system(size: 10, weight: .semibold))
            .tracking(0.3)
            .padding(.horizontal, 5).padding(.vertical, 2)
            .background(color.opacity(0.16), in: RoundedRectangle(cornerRadius: 3))
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
    var spacing: CGFloat = 6

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
