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
    @Binding var title: String

    func makeCoordinator() -> Coordinator { Coordinator(port: port, title: $title) }

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
        private var socket: URLSessionWebSocketTask?
        private weak var view: TerminalView?
        @Binding private var title: String

        init(port: UInt16, title: Binding<String>) {
            self.port = port
            _title = title
        }

        func attach(_ view: TerminalView) {
            self.view = view
            let url = URL(string: "ws://127.0.0.1:\(port)/api/term/ws")!
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
