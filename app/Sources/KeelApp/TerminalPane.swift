import AppKit
import SwiftTerm
import SwiftUI

/// A real terminal, over the daemon's PTY.
///
/// The framing is the daemon's own, from `term.rs`: the client sends binary frames of keystrokes
/// and one text frame `"\0{cols},{rows}"` to resize; the server sends binary frames of PTY output
/// and **text frames carrying the tab title**, which is the foreground process name it polls for.
/// Anything that assumes text frames are output will print the word `zsh` into the shell.
struct TerminalPane: NSViewRepresentable {
    let port: UInt16
    /// The lane's checkout to open the shell in, if it has one.
    var worktree: String? = nil
    @Binding var title: String

    func makeCoordinator() -> Coordinator { Coordinator(port: port, worktree: worktree, title: $title) }

    func makeNSView(context: Context) -> TerminalView {
        let view = TerminalView(frame: .zero)
        view.terminalDelegate = context.coordinator
        context.coordinator.attach(view)
        return view
    }

    func updateNSView(_ view: TerminalView, context: Context) {}

    @MainActor
    final class Coordinator: NSObject, @MainActor TerminalViewDelegate {
        private let port: UInt16
        private let worktree: String?
        private var socket: URLSessionWebSocketTask?
        private weak var view: TerminalView?
        @Binding private var title: String

        init(port: UInt16, worktree: String?, title: Binding<String>) {
            self.port = port
            self.worktree = worktree
            _title = title
        }

        /// A command handed to the terminal from elsewhere — a "Run in Terminal" button. Typed
        /// with a newline, into the shell, exactly as the person would have.
        func type(_ command: String) {
            socket?.send(.data(Data((command + "\n").utf8))) { _ in }
        }

        func attach(_ view: TerminalView) {
            self.view = view
            NotificationCenter.default.addObserver(forName: .keelRunInTerminal, object: nil,
                                                   queue: .main) { [weak self] note in
                guard let cmd = note.object as? String else { return }
                // The pane may have just been opened for this; give the socket a moment.
                Task { @MainActor [weak self] in
                    try? await Task.sleep(for: .milliseconds(self?.socket == nil ? 600 : 50))
                    self?.type(cmd)
                }
            }
            let url = URL(string: "ws://127.0.0.1:\(port)/api/term/ws"
                          + (worktree.map { "?wt=" + $0 } ?? ""))!
            let task = URLSession.shared.webSocketTask(with: url)
            socket = task
            task.resume()
            receive()
        }

        private func receive() {
            socket?.receive { [weak self] result in
                guard let self else { return }
                Task { @MainActor in
                    switch result {
                    case .success(.data(let d)):
                        self.view?.feed(byteArray: ArraySlice(d))
                    case .success(.string(let s)):
                        // A text frame is the tab title, never output.
                        self.title = s
                    case .success:
                        break
                    case .failure:
                        return
                    }
                    self.receive()
                }
            }
        }

        // MARK: TerminalViewDelegate

        func send(source: TerminalView, data: ArraySlice<UInt8>) {
            socket?.send(.data(Data(data))) { _ in }
        }

        func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
            // The NUL prefix is what marks this as a resize rather than someone typing the digits.
            socket?.send(.string("\u{0}\(newCols),\(newRows)")) { _ in }
        }

        func setTerminalTitle(source: TerminalView, title: String) { self.title = title }
        func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
        func scrolled(source: TerminalView, position: Double) {}
        func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}
        func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {
            guard let url = URL(string: link), url.scheme == "http" || url.scheme == "https" else { return }
            NSWorkspace.shared.open(url)
        }
        func bell(source: TerminalView) {}
        func clipboardCopy(source: TerminalView, content: Data) {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(String(decoding: content, as: UTF8.self), forType: .string)
        }
        func iTermContent(source: TerminalView, content: ArraySlice<UInt8>) {}
    }
}
