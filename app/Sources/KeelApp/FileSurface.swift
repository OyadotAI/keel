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

    private static let maxLines = 5000

    private var isImage: Bool {
        UTType(filenameExtension: (path as NSString).pathExtension)?.conforms(to: .image) ?? false
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: K.S.sm) {
                Image(systemName: "doc.text").font(.system(size: 10)).foregroundStyle(K.C.faint)
                Text(path).font(K.F.mono(11.5, .medium)).foregroundStyle(K.C.text)
                    .lineLimit(1).truncationMode(.head).textSelection(.enabled)
                if let data { Text(SessionModel.humanSize(data.count)).font(K.F.mono(10)).foregroundStyle(K.C.faint) }
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
            loading = true
            data = await model.raw(path)
            loading = false
        }
    }

    @ViewBuilder
    private var content: some View {
        if loading {
            Text("Reading…").font(K.F.small).foregroundStyle(K.C.faint)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let data, isImage, let image = NSImage(data: data) {
            ScrollView([.vertical, .horizontal]) {
                Image(nsImage: image).padding(K.S.lg)
            }
        } else if let data, let text = String(data: data, encoding: .utf8) {
            let lines = text.split(omittingEmptySubsequences: false, whereSeparator: \.isNewline)
            let shown = lines.prefix(Self.maxLines)
            GeometryReader { geo in
                ScrollView([.vertical, .horizontal]) {
                    // A plain VStack: a lazy one sizes rows to the proposed width, so every
                    // line past the pane's edge was ellipsised instead of scrolling. The file
                    // is capped at maxLines, so eagerness costs nothing.
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(shown.enumerated()), id: \.offset) { i, line in
                            HStack(alignment: .top, spacing: 0) {
                                Text("\(i + 1)").frame(width: 44, alignment: .trailing)
                                    .foregroundStyle(K.C.faint).padding(.trailing, K.S.sm)
                                Text(line.isEmpty ? " " : String(line))
                                    .foregroundStyle(K.C.text).textSelection(.enabled).lineLimit(1)
                                    .fixedSize(horizontal: true, vertical: false)
                            }
                            .font(K.F.code)
                        }
                        if lines.count > Self.maxLines {
                            Text("… \(lines.count - Self.maxLines) more lines. Attach the file to give the agent all of it.")
                                .font(K.F.micro).foregroundStyle(K.C.faint).padding(K.S.md)
                        }
                    }
                    .padding(.vertical, K.S.sm)
                    .frame(minWidth: max(geo.size.width, 1), minHeight: max(geo.size.height, 1),
                           alignment: .topLeading)
                }
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
