import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// Things you paste or drop into the composer.
///
/// An attachment is a file on disk that rides along as an `@path` mention, which is the convention
/// Claude Code already uses and the daemon already prefixes. Nothing about the chat protocol
/// changes: the Read tool returns PNG and JPEG as visual content, so a path is enough to show the
/// agent a screenshot.
///
/// Long text goes through the same door for a less obvious reason. The prompt travels as a GET
/// query parameter and then as an argv value, so a big paste has to survive a URL length limit and
/// then macOS's ~256 KB `ARG_MAX` — and it does not. A file dodges both.
struct Attachment: Identifiable, Hashable {
    let id = UUID()
    /// Repo-relative, as returned by `/api/attach`.
    let path: String
    let label: String
    let thumbnail: NSImage?

    func hash(into h: inout Hasher) { h.combine(id) }
    static func == (a: Attachment, b: Attachment) -> Bool { a.id == b.id }
}

extension SessionModel {
    /// Text longer than this is filed rather than typed. Chosen to be well clear of anything
    /// someone composes by hand and well under the limits above.
    static let longPaste = 1500

    struct Attached: Decodable { var path: String; var bytes: Int }

    func attach(data: Data, name: String, thumbnail: NSImage? = nil, label: String? = nil) {
        Task { [client] in
            do {
                var req = URLRequest(url: await client.base
                    .appendingPathComponent("api/attach"))
                req.url = URL(string: req.url!.absoluteString
                    + "?name=" + (name.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? "file"))
                req.httpMethod = "POST"
                req.httpBody = data
                let (body, response) = try await URLSession.shared.data(for: req)
                guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
                    self.lastError = String(decoding: body, as: UTF8.self)
                    return
                }
                let a = try JSONDecoder().decode(Attached.self, from: body)
                self.attachments.append(Attachment(
                    path: a.path,
                    label: label ?? "\(name) · \(Self.humanSize(a.bytes))",
                    thumbnail: thumbnail))
            } catch {
                self.lastError = error.localizedDescription
            }
        }
    }

    static func humanSize(_ bytes: Int) -> String {
        bytes < 1024 ? "\(bytes) B"
            : bytes < 1024 * 1024 ? "\(bytes / 1024) KB"
            : String(format: "%.1f MB", Double(bytes) / 1024 / 1024)
    }

    /// Handle a paste. Returns true when the paste was taken as an attachment and should not also
    /// land in the text box.
    @discardableResult
    func takePaste(_ board: NSPasteboard) -> Bool {
        // Images first: a screenshot on the pasteboard is also available as text on some apps, and
        // the picture is always the thing that was meant.
        if let images = board.readObjects(forClasses: [NSImage.self]) as? [NSImage], !images.isEmpty {
            for image in images {
                guard let tiff = image.tiffRepresentation,
                      let png = NSBitmapImageRep(data: tiff)?
                        .representation(using: .png, properties: [:]) else { continue }
                attach(data: png, name: "pasted.png", thumbnail: image)
            }
            return true
        }
        if let urls = board.readObjects(forClasses: [NSURL.self]) as? [URL], !urls.isEmpty {
            for url in urls { attach(fileURL: url) }
            return true
        }
        if let text = board.string(forType: .string), text.count > Self.longPaste {
            let chars = text.count
            attach(data: Data(text.utf8),
                   name: "paste-\(chars)-chars.txt",
                   label: "pasted text · \(chars.formatted()) chars")
            return true
        }
        return false
    }

    func attach(fileURL url: URL) {
        guard let data = try? Data(contentsOf: url) else { return }
        let image = NSImage(contentsOf: url)
        attach(data: data, name: url.lastPathComponent, thumbnail: image)
    }

    /// Attachments become `@path` mentions ahead of the prompt — the same shape the web UI sends,
    /// so a session reads the same either way.
    func promptWithAttachments(_ text: String) -> String {
        guard !attachments.isEmpty else { return text }
        return attachments.map { "@" + $0.path }.joined(separator: " ") + "\n\n" + text
    }
}

/// The chips above the composer.
struct AttachmentStrip: View {
    @Bindable var model: SessionModel

    var body: some View {
        if !model.attachments.isEmpty {
            Flow(spacing: 6) {
                ForEach(model.attachments) { a in
                    HStack(spacing: 5) {
                        if let t = a.thumbnail {
                            Image(nsImage: t).resizable().scaledToFill()
                                .frame(width: 16, height: 16).clipped()
                                .clipShape(RoundedRectangle(cornerRadius: 2))
                        } else {
                            Image(systemName: "paperclip")
                                .font(.system(size: 8)).foregroundStyle(K.C.faint)
                        }
                        Text(a.label)
                            .font(K.F.mono(10)).foregroundStyle(K.C.dim)
                            .lineLimit(1).truncationMode(.head)
                        CloseButton(size: 8) {
                            model.attachments.removeAll { $0.id == a.id }
                        }
                    }
                    .padding(.leading, 6).padding(.trailing, 2)
                    .padding(.vertical, 1)
                    .background(K.C.raised, in: RoundedRectangle(cornerRadius: K.R.sm))
                    .overlay(
                        RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1)
                    )
                    // A second way out, for when the pointer is not the fastest route.
                    .contextMenu {
                        Button("Remove") { model.attachments.removeAll { $0.id == a.id } }
                        Button("Remove all") { model.attachments.removeAll() }
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
