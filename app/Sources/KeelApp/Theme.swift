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

    /// Every button is this tall.
    ///
    /// A label that wraps makes its own button taller and leaves the others centred against it —
    /// which is what a row of answers looked like the moment one of them named four rules. One
    /// height, one line, and the long ones truncate.
    static let buttonHeight: CGFloat = 24

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
        /// Something arriving that the person has to deal with — a question, an approval.
        ///
        /// The only spring in the app, and a heavily damped one: this is the single case where
        /// the motion is the point. A card carrying four decisions that snaps into existence in
        /// 0.12s reads as a glitch, and one that overshoots reads as a toy. `response` is the
        /// travel time; the damping is just under critical, so it settles without a bounce.
        static let enter = Animation.spring(response: 0.34, dampingFraction: 0.86)
    }
}

// MARK: - Shared chrome

/// A quiet section header in a rail. Sentence case reads like navigation rather than telemetry.
struct RailHeader: View {
    let title: String
    var trailing: String?
    /// Only the panel's own header has one: a way out that does not require knowing the rail icon
    /// toggles. ⌘⇧E does it too, and a shortcut is not an affordance.
    var onClose: (() -> Void)?

    init(_ title: String, trailing: String? = nil, onClose: (() -> Void)? = nil) {
        self.title = title
        self.trailing = trailing
        self.onClose = onClose
    }

    var body: some View {
        HStack(spacing: K.S.xs) {
            Text(title)
                .font(K.F.small.weight(.medium))
                .foregroundStyle(K.C.dim)
            Spacer()
            if let trailing {
                Text(trailing).font(K.F.mono(10)).monospacedDigit().foregroundStyle(K.C.dim)
            }
            if let onClose {
                CloseButton(label: "Close the \(title) panel (⌘⇧E)", action: onClose)
                    .padding(.leading, K.S.xxs)
            }
        }
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
            .lineLimit(1)
            .foregroundStyle(enabled ? Color.white : K.C.faint)
            .padding(.horizontal, K.S.md)
            .frame(height: K.buttonHeight)
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
            .lineLimit(1)
            .truncationMode(.middle)
            .foregroundStyle(configuration.isPressed ? K.C.text : tone)
            .padding(.horizontal, K.S.sm)
            .frame(height: K.buttonHeight)
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

/// Nothing here yet, said once, the same way everywhere.
///
/// There were four of these — `Blank` in the extension panels, a private `Empty` in `SidePanel`,
/// `EmptyStage` in the trace, and a one-line `empty(_:)` in the review packet — plus about ten
/// bare `Text`s. They disagreed on the icon, the inset, the type and whether there was anything to
/// do about it, so five panels looked like one product and five looked like five.
///
/// Every field but the message is optional, which is the whole range: a panel that can offer an
/// action gets the full shape, and a line of explanation is the same component with the rest left
/// out. The inset matches `RailHeader`'s, so the first thing in a panel lines up with its title
/// whether that thing is a list or an apology.
struct EmptyState: View {
    var icon: String? = nil
    var title: String? = nil
    let message: String
    var actionLabel: String? = nil
    var action: (() -> Void)? = nil

    init(icon: String? = nil, title: String? = nil, _ message: String,
         actionLabel: String? = nil, action: (() -> Void)? = nil) {
        self.icon = icon
        self.title = title
        self.message = message
        self.actionLabel = actionLabel
        self.action = action
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            if let icon {
                Image(systemName: icon)
                    .font(K.F.ui(16, .medium)).foregroundStyle(K.C.accent)
                    .frame(width: 24, height: 24, alignment: .leading)
                    // The title says what this says. Left visible, it is announced as its SF
                    // Symbol name before the sentence anyone actually needs.
                    .accessibilityHidden(true)
            }
            if let title {
                Text(title).font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
            }
            // `.init` so a message can carry a `**bold**` or a `\`path\``, which several do.
            Text(.init(message))
                .font(K.F.small).foregroundStyle(K.C.dim)
                .fixedSize(horizontal: false, vertical: true)
            if let actionLabel, let action {
                Button(actionLabel, action: action)
                    .buttonStyle(QuietButton(tone: K.C.accent))
                    .padding(.top, K.S.xs)
            }
        }
        .padding(.horizontal, K.S.md)
        .padding(.vertical, K.S.lg)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Waiting on the daemon, said out loud.
///
/// An empty list and a list that has not arrived look identical, and five surfaces had no way to
/// tell them apart — `SkillCatalog`, the clone list, `PairingSettings`, `FileTree` and the session
/// list all rendered "nothing" while a request was still in flight. The five idioms this replaces
/// included three separate copies of the string `"Reading…"`.
struct Loading: View {
    let what: String

    init(_ what: String = "Reading…") { self.what = what }

    var body: some View {
        HStack(spacing: K.S.sm) {
            ProgressView().controlSize(.mini)
            Text(what).font(K.F.small).foregroundStyle(K.C.faint)
        }
        .padding(.horizontal, K.S.md)
        .padding(.vertical, K.S.lg)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Something went wrong, in the place it went wrong.
///
/// `model.lastError` is one string on the model, and four surfaces rendered it at three different
/// sizes — so a single git failure could appear three times on screen at once, in three type
/// treatments. One shape, one place, and a way to put it away.
/// What to do about a failure.
///
/// The repository already insists that every scanner finding carries a `Fix`, because "a finding
/// without one turns the report into a lint run nobody acts on". A failure at runtime is the same
/// thing: "the isolated checkout could not be created" is a wall, and "this folder is not a git
/// repository — [Initialise git]" is a next step.
struct Fix {
    let label: String
    let run: () -> Void
}

struct ErrorRow: View {
    let message: String
    var fix: Fix? = nil
    var dismiss: (() -> Void)? = nil

    var body: some View {
        HStack(alignment: .top, spacing: K.S.sm) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(K.F.tiny).foregroundStyle(K.C.del)
                .padding(.top, K.S.xxs)
                .accessibilityHidden(true)
            Text(message).font(K.F.small).foregroundStyle(K.C.del)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
            Spacer(minLength: 0)
            if let fix {
                Button(fix.label, action: fix.run)
                    .buttonStyle(FilledButton(tone: K.C.del))
            }
            if let dismiss { CloseButton(label: "Dismiss this error", action: dismiss) }
        }
        .padding(.horizontal, K.S.sm).padding(.vertical, K.S.half)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(K.C.del.wash, in: RoundedRectangle(cornerRadius: K.R.sm))
    }
}

/// The filter at the top of a panel.
///
/// There were two, and they disagreed about everything a person can notice: one used a
/// magnifying glass and one a filter glyph, one set 12pt and one 11pt mono, they had different
/// insets, and only one of them could be cleared.
struct SearchField: View {
    let prompt: String
    @Binding var text: String
    var icon = "magnifyingglass"

    var body: some View {
        HStack(spacing: K.S.xs) {
            Image(systemName: icon).font(K.F.tiny).foregroundStyle(K.C.faint)
                .accessibilityHidden(true)
            TextField(prompt, text: $text).textFieldStyle(.plain).font(K.F.small)
            if !text.isEmpty { CloseButton(label: "Clear the filter") { text = "" } }
        }
        .padding(.horizontal, K.S.sm).padding(.vertical, K.S.tight)
        .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
        .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
        // Bottom padding as well as top: without it the first row of the list sat hard against
        // the field, and the two read as one control.
        .padding(.horizontal, K.S.md)
        .padding(.top, K.S.sm).padding(.bottom, K.S.md)
    }
}

/// Scrolling up releases autoscroll; this is the way back down.
///
/// The conversation and the trace each carried their own identical copy of this — twenty lines,
/// down to the shadow radius — because there was nowhere shared to put it.
struct JumpToLatest: View {
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: K.S.snug) {
                Image(systemName: "arrow.down").font(K.F.tiny.weight(.bold))
                Text("Jump to latest").font(K.F.micro)
            }
            .padding(.horizontal, K.S.sm).padding(.vertical, K.S.snug)
            .background(K.C.raised, in: Capsule())
            .overlay(Capsule().stroke(K.C.lineStrong, lineWidth: 1))
            .shadow(color: .black.opacity(0.25), radius: 8, y: 2)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .foregroundStyle(K.C.text)
        .padding(.bottom, K.S.md)
    }
}

/// A group inside a panel: a chevron, a name, how many, and a rule above it.
///
/// Lifted out of `GitPanel`, which is the only panel that had it and the only panel that read as
/// designed. Everything else was a flat list under a header, so Git looked like a product and its
/// siblings looked like debug output.
///
/// Only for a panel with *several* groups. One always-open section is a chevron that does nothing,
/// and a second header repeating the count already in the panel's own title.
struct PanelSection<Content: View>: View {
    let title: String
    var count: Int?
    @Binding var open: Bool
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(K.M.flow) { open.toggle() }
            } label: {
                HStack(spacing: K.S.sm) {
                    Image(systemName: "chevron.right")
                        .font(K.F.ui(9, .semibold)).frame(width: 10)
                        .rotationEffect(.degrees(open ? 90 : 0))
                        .foregroundStyle(K.C.faint)
                    Text(title).font(K.F.small.weight(.semibold))
                    Spacer()
                    if let count {
                        Text("\(count)").font(K.F.codeTiny).foregroundStyle(K.C.faint)
                    }
                }
                .foregroundStyle(K.C.text)
                .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityAddTraits(open ? .isSelected : [])
            if open { content.transition(.opacity) }
        }
        .overlay(alignment: .top) { Hairline() }
    }
}

/// One thing in a panel: what it is called, what it is for, and that clicking opens it.
///
/// The detail was set in monospace at the faintest ink in the palette, which is the treatment for
/// a path or a command — these are sentences. Two lines of prose also need more than the 3pt of
/// vertical padding a one-line row wants, or the rows run together into a wall.
struct PanelRow: View {
    let name: String
    var detail: String = ""
    /// Anything that arrived with the repository was written by whoever wrote the repository.
    var fromRepo = false
    var dimmed = false
    var selected = false
    /// Shown when the row opens something, which is the only affordance saying that it does.
    var opens = true
    /// Set when the detail is a command or a path rather than a sentence. Mono for one, prose for
    /// the other — a description in monospace reads as output, and a command in prose reads as a
    /// claim about what it does rather than the thing that will run.
    var code = false
    let action: () -> Void

    var body: some View {
        HoverRow(selected: selected) {
            HStack(spacing: K.S.sm) {
                VStack(alignment: .leading, spacing: K.S.hair) {
                    HStack(spacing: K.S.half) {
                        Text(name)
                            .font(K.F.small.weight(.medium))
                            .foregroundStyle(dimmed ? K.C.faint : K.C.text)
                            .lineLimit(1)
                        if fromRepo { Pill(text: "REPO", tone: .warn) }
                    }
                    if !detail.isEmpty {
                        Text(detail)
                            .font(code ? K.F.codeTiny : K.F.tiny)
                            .foregroundStyle(K.C.dim)
                            .lineLimit(1).truncationMode(code ? .head : .tail)
                    }
                }
                Spacer(minLength: 0)
                if opens {
                    Image(systemName: "chevron.right")
                        .font(K.F.ui(9, .semibold)).foregroundStyle(K.C.faint)
                        .accessibilityHidden(true)
                }
            }
            .padding(.vertical, K.S.tight)
        } action: {
            action()
        }
        .hint(detail.isEmpty ? name : "\(name) — \(detail)")
    }
}

/// The one thing a panel offers when you have read its list: always last, always after a rule.
struct PanelFooter: View {
    let title: String
    var icon = "plus"
    let action: () -> Void

    init(_ title: String, icon: String = "plus", action: @escaping () -> Void) {
        self.title = title
        self.icon = icon
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            HStack(spacing: K.S.half) {
                Image(systemName: icon).font(K.F.ui(9, .semibold))
                Text(title).font(K.F.small)
                Spacer(minLength: 0)
            }
            .foregroundStyle(K.C.accent)
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .overlay(alignment: .top) { Hairline() }
    }
}

/// A group in Settings: what it is called, the controls, and the sentence that says what they do.
///
/// Three of the five settings panes were `Form` with `.formStyle(.grouped)` — AppKit's System
/// Settings look, with its own inset, its own type and its own grey rounded boxes — and the other
/// two were hand-rolled `VStack`s, flush left with none of that. Side by side they read as two
/// applications, which is the exact complaint the `Field` modifier below was written to fix and
/// only fixed for text fields.
///
/// This is `PanelSection`'s shape without the disclosure, because a settings group is always open.
struct SettingsSection<Content: View>: View {
    let title: String
    /// The sentence under the controls. Settings is where somebody goes when they are unsure, so
    /// the explanation is part of the group rather than a tooltip on it.
    var note: String?
    @ViewBuilder var content: Content

    init(_ title: String, note: String? = nil, @ViewBuilder content: () -> Content) {
        self.title = title
        self.note = note
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.sm) {
            Text(title).font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
            content
            if let note {
                Text(.init(note))
                    .font(K.F.small).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.vertical, K.S.lg)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .top) { Hairline() }
    }
}

/// A switch in Settings: what it does on the left, the switch on the right, the consequence below.
///
/// The two hand-rolled panes put the switch immediately after its label, so it landed in a
/// different place on every row and the eye had to hunt for it.
struct SettingsToggle: View {
    let title: String
    var note: String?
    @Binding var isOn: Bool

    init(_ title: String, note: String? = nil, isOn: Binding<Bool>) {
        self.title = title
        self.note = note
        self._isOn = isOn
    }

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.xs) {
            HStack(spacing: K.S.sm) {
                Text(title).font(K.F.body).foregroundStyle(K.C.text)
                Spacer(minLength: K.S.md)
                Toggle("", isOn: $isOn)
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .controlSize(.small)
                    .accessibilityLabel(title)
            }
            if let note {
                Text(.init(note))
                    .font(K.F.small).foregroundStyle(K.C.dim)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// One rule, one tool, one device: a name on the left and what you can do about it on the right.
struct SettingsRow<Trailing: View>: View {
    let title: String
    var detail: String?
    var code = false
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: K.S.sm) {
            VStack(alignment: .leading, spacing: K.S.hair) {
                Text(title)
                    .font(code ? K.F.code : K.F.body)
                    .foregroundStyle(K.C.text)
                    .textSelection(.enabled)
                if let detail {
                    Text(detail).font(K.F.small).foregroundStyle(K.C.dim)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: K.S.sm)
            trailing
        }
        .padding(.vertical, K.S.xs)
        .frame(maxWidth: .infinity, alignment: .leading)
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
    func fixed(_ v: Double, _ places: Int) -> String {
        v.formatted(.number.precision(.fractionLength(places)))
    }
    switch n {
    case ..<1000: return "\(n)"
    case ..<1_000_000: return fixed(Double(n) / 1000, 1) + "k"
    default: return fixed(Double(n) / 1_000_000, 2) + "M"
    }
}

/// What a turn or a session cost, in the currency the provider actually bills in.
///
/// `.currency(code:)` rather than a `$` and a number: it is locale-aware in both directions, so a
/// Mac set to French renders `0,004 $US` — the right separator *and* unambiguous about which
/// dollar. `String(format: "$%.3f")`, which this replaces, formatted in the C locale regardless,
/// so every cost on screen used a decimal point in a UI where nothing else did.
///
/// Three fraction digits because a turn frequently costs less than a cent, and a cost that always
/// reads `$0.00` is not a cost.
func money(_ amount: Double, places: Int = 3) -> String {
    amount.formatted(.currency(code: "USD").precision(.fractionLength(places)))
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
