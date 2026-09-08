import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// A file, read-only, in the stage.
///
/// Keel does not edit files — that is what the editor you already have is for — but a click on
/// a file has to *show* it. Text with line numbers, an image as an image, and a button to hand
/// the file to the agent as context, which is what the click used to do silently.
struct FileSurface: View {
    let model: SessionModel
    let path: String
    @State private var data: Data?
    @State private var loading = true
    private let fixtureData: Data?

    init(model: SessionModel, path: String, data: Data? = nil) {
        self.model = model
        self.path = path
        fixtureData = data
        _data = State(initialValue: data)
        _loading = State(initialValue: data == nil)
    }

    private static let maxLines = 5000

    private var isImage: Bool {
        UTType(filenameExtension: (path as NSString).pathExtension)?.conforms(to: .image) ?? false
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "doc.text").font(K.F.tiny).foregroundStyle(K.C.faint)
                Text(path).font(K.F.codeSmall.weight(.medium)).foregroundStyle(K.C.text)
                    .lineLimit(1).truncationMode(.head).textSelection(.enabled)
                if let data { Text(SessionModel.humanSize(data.count)).font(K.F.codeTiny).foregroundStyle(K.C.faint) }
                Spacer()
                Button("Attach as context") { model.mention(path) }
                    .buttonStyle(QuietButton(tone: K.C.accent))
                    .help("Send this file along with the next prompt, as @\(path)")
                Button("Reveal") { Task { await model.reveal(path) } }.buttonStyle(QuietButton())
                CloseButton(label: "Close file") { model.viewingFile = nil }
            }
            .padding(.horizontal, K.S.md).padding(.vertical, K.S.sm)
            .background(K.C.surface)
            Hairline()
            content
        }
        .background(K.C.bg)
        .task(id: path) {
            guard fixtureData == nil else { return }
            loading = true
            let fresh = await model.raw(path)
            guard !Task.isCancelled else { return }
            data = fresh
            loading = false
        }
    }

    @ViewBuilder
    private var content: some View {
        if loading {
            Loading()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let data, isImage, let image = NSImage(data: data) {
            ScrollView([.vertical, .horizontal]) {
                Image(nsImage: image).padding(K.S.lg)
            }
        } else if let data, let text = String(data: data, encoding: .utf8) {
            let lines = text.split(omittingEmptySubsequences: false, whereSeparator: \.isNewline)
            let shown = lines.prefix(Self.maxLines)
            SourceTextView(text: shown.joined(separator: "\n"))
            if lines.count > Self.maxLines {
                Text("… \(lines.count - Self.maxLines) more lines. Attach the file to give the agent all of it.")
                    .font(K.F.micro).foregroundStyle(K.C.faint).padding(K.S.md)
            }
        } else if data != nil {
            Text("Binary file — nothing to show. Attach it and the agent can read it.")
                .font(K.F.small).foregroundStyle(K.C.faint)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            Text("Could not read this file.").font(K.F.small).foregroundStyle(K.C.del)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

/// One AppKit text document, not thousands of SwiftUI layout engines. TextKit lays out the
/// visible text and owns horizontal scrolling, selection, and copying without truncating lines.
struct SourceTextView: NSViewRepresentable {
    let text: String

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        let view = NSTextView(frame: .zero)
        view.isEditable = false
        view.isSelectable = true
        view.isRichText = false
        view.isVerticallyResizable = true
        view.isHorizontallyResizable = true
        view.minSize = .zero
        view.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        view.textContainer?.containerSize = view.maxSize
        view.textContainer?.widthTracksTextView = false
        view.textContainerInset = NSSize(width: 8, height: 8)
        view.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        view.setAccessibilityLabel("File contents, read only")
        scroll.documentView = view
        scroll.verticalRulerView = SourceLineRuler(scrollView: scroll, orientation: .verticalRuler)
        scroll.hasVerticalRuler = true
        scroll.rulersVisible = true
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let view = scroll.documentView as? NSTextView else { return }
        view.textColor = NSColor(K.C.text)
        view.backgroundColor = NSColor(K.C.bg)
        scroll.backgroundColor = NSColor(K.C.bg)
        if view.string != text {
            view.string = text
            if let container = view.textContainer, let manager = view.layoutManager {
                manager.ensureLayout(for: container)
                let used = manager.usedRect(for: container)
                view.setFrameSize(NSSize(width: max(scroll.contentSize.width, used.width + 2 * view.textContainerInset.width),
                                         height: max(scroll.contentSize.height, used.height + 2 * view.textContainerInset.height)))
            }
            (scroll.verticalRulerView as? SourceLineRuler)?.setText(text)
            view.scrollToBeginningOfDocument(nil)
        }
    }
}

private final class SourceLineRuler: NSRulerView {
    private var starts = [0]
    override init(scrollView: NSScrollView?, orientation: NSRulerView.Orientation) {
        super.init(scrollView: scrollView, orientation: orientation)
        ruleThickness = 48
        clientView = scrollView?.documentView
    }
    required init(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    func setText(_ text: String) {
        starts = [0]
        for (i, unit) in text.utf16.enumerated() where unit == 10 { starts.append(i + 1) }
        needsDisplay = true
    }

    override func drawHashMarksAndLabels(in rect: NSRect) {
        guard let view = clientView as? NSTextView,
              let manager = view.layoutManager, let container = view.textContainer,
              let scrollView else { return }
        NSColor(K.C.surface).setFill()
        bounds.fill()
        let visible = view.visibleRect.offsetBy(dx: -view.textContainerOrigin.x, dy: -view.textContainerOrigin.y)
        let glyphs = manager.glyphRange(forBoundingRect: visible, in: container)
        manager.enumerateLineFragments(forGlyphRange: glyphs) { [self] fragment, _, _, range, _ in
            let character = manager.characterIndexForGlyph(at: range.location)
            var low = 0, high = starts.count
            while low < high {
                let middle = (low + high) / 2
                if starts[middle] <= character { low = middle + 1 } else { high = middle }
            }
            let label = "\(low)" as NSString
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .regular),
                .foregroundColor: NSColor(K.C.faint)]
            let y = fragment.minY + view.textContainerOrigin.y - scrollView.contentView.bounds.minY
            label.draw(at: NSPoint(x: ruleThickness - label.size(withAttributes: attrs).width - 8, y: y), withAttributes: attrs)
        }
    }
}
