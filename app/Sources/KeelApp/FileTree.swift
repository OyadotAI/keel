import SwiftUI

/// The repository, as a tree.
///
/// Read once per project from `/api/tree`, which returns the whole thing in one payload — the
/// daemon already excludes `.git`, `.keel`, `target` and `node_modules`, so this is the repository
/// as someone thinks of it rather than as the filesystem holds it.
struct FileTree: View {
    let model: SessionModel
    @State private var filter = ""

    var body: some View {
        VStack(alignment: .leading, spacing: K.S.xxs) {
            HStack(spacing: K.S.xs) {
                Image(systemName: "line.3.horizontal.decrease")
                    .font(.system(size: 10)).foregroundStyle(K.C.faint)
                TextField("Filter", text: $filter)
                    .textFieldStyle(.plain)
                    .font(K.F.mono(11))
            }
            .padding(.horizontal, K.S.sm).padding(.vertical, 3)
            .background(K.C.well, in: RoundedRectangle(cornerRadius: K.R.sm))
            .overlay(RoundedRectangle(cornerRadius: K.R.sm).stroke(K.C.line, lineWidth: 1))
            .padding(.horizontal, K.S.sm)
            .padding(.bottom, K.S.xs)

            if filter.isEmpty {
                if model.tree.isEmpty {
                    VStack(alignment: .leading, spacing: K.S.xs) {
                        Label("No repository files", systemImage: "folder")
                            .font(K.F.small.weight(.semibold)).foregroundStyle(K.C.text)
                        Text("Files appear after the project tree finishes loading. Generated and ignored directories stay hidden.")
                            .font(K.F.micro).foregroundStyle(K.C.dim)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(K.S.md)
                    .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    ForEach(model.tree) { node in
                        TreeRow(node: node, depth: 0, model: model)
                    }
                }
            } else {
                // Filtering flattens: a match three folders down is not easier to find because
                // its ancestors are still drawn above it.
                ForEach(model.files.filter { $0.localizedCaseInsensitiveContains(filter) }.prefix(80), id: \.self) { path in
                    HoverRow {
                        HStack(spacing: 0) {
                            Text((path as NSString).deletingLastPathComponent + "/")
                                .foregroundStyle(K.C.faint)
                            Text((path as NSString).lastPathComponent)
                                .foregroundStyle(K.C.dim)
                        }
                        .font(K.F.mono(11))
                        .lineLimit(1).truncationMode(.head)
                    } action: {
                        model.show(file: path)
                    }
                }
            }
        }
    }
}

private struct TreeRow: View {
    let node: Wire.Node
    let depth: Int
    let model: SessionModel
    @State private var open = false

    var body: some View {
        HoverRow {
            HStack(spacing: K.S.xs) {
                if node.dir {
                    Image(systemName: open ? "chevron.down" : "chevron.right")
                        .font(.system(size: 7, weight: .bold))
                        .foregroundStyle(K.C.faint).frame(width: 8)
                } else {
                    Spacer().frame(width: 8)
                }
                Image(systemName: node.dir ? "folder.fill" : icon(node.name))
                    .font(.system(size: 10))
                    .foregroundStyle(node.dir ? K.C.faint : K.C.faint.opacity(0.7))
                    .frame(width: 11)
                Text(node.name)
                    .font(K.F.small)
                    .foregroundStyle(node.dir ? K.C.dim : K.C.text)
                    .lineLimit(1).truncationMode(.middle)
                Spacer()
            }
            .padding(.leading, CGFloat(depth) * 11)
        } action: {
            if node.dir { withAnimation(K.M.quick) { open.toggle() } } else {
                // Open it. Attaching is the menu; a click that silently added context looked
                // like a click that did nothing.
                model.show(file: node.path)
            }
        }
        .contextMenu {
            Button("Attach as context") { model.mention(node.path) }
            Button("Reveal in Finder") { Task { await model.reveal(node.path) } }
            Divider()
            // No confirmation: the daemon moves it to the Trash rather than unlinking it, so the
            // undo is the one macOS already has.
            Button("Move to Trash", role: .destructive) {
                Task { await model.delete(node.path) }
            }
        }

        if open, let children = node.children {
            ForEach(children) { child in
                TreeRow(node: child, depth: depth + 1, model: model)
            }
        }
    }
}


/// A glyph per family. Not a full icon set — just enough that a tree scans by shape as well as by
/// name, which is most of what an icon in a file list is for.
func icon(_ name: String) -> String {
    let ext = (name as NSString).pathExtension.lowercased()
    switch ext {
    case "swift", "rs", "go", "py", "rb", "java", "kt", "c", "h", "cpp": return "chevron.left.forwardslash.chevron.right"
    case "ts", "tsx", "js", "jsx", "mjs": return "curlybraces"
    case "json", "toml", "yaml", "yml", "lock", "plist": return "list.bullet.rectangle"
    case "md", "markdown", "txt": return "doc.text"
    case "png", "jpg", "jpeg", "gif", "svg", "webp": return "photo"
    case "sh", "bash", "zsh", "fish": return "terminal"
    case "html", "css", "scss": return "globe"
    default: return "doc"
    }
}
