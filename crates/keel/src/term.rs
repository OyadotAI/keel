//! A real terminal.
//!
//! Not a command runner: a PTY running the user's own shell. Anything less fails the moment you
//! need an interactive prompt, a pager, colour, or a long-running process — which is most of what a
//! terminal is for.
//!
//! The socket is bidirectional and byte-oriented. Terminal output is not line-structured, and
//! buffering it into lines would break every progress bar and prompt.

use axum::extract::{
    State,
    ws::{Message, WebSocket, WebSocketUpgrade},
};
use axum::response::Response;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::sync::Arc;

use crate::serve::AppState;

/// The shell to run. Honour the user's own, since their prompt, aliases and PATH live there.
fn shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

pub async fn ws(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    let repo = state.repo();
    ws.on_upgrade(move |socket| session(socket, repo.to_string()))
}

/// What the browser can ask the pty to do.
enum Cmd {
    Input(Vec<u8>),
    Resize(u16, u16),
}

async fn session(socket: WebSocket, cwd: String) {
    use futures_util::{SinkExt, StreamExt};
    let (mut sender, mut receiver) = socket.split();

    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Cmd>();

    // The pty master is Send but not Sync, so it cannot live in an async task that awaits. One
    // owning thread holds it, reads from it, and applies commands; the async side only moves bytes.
    let spawned = std::thread::spawn(move || pty_thread(&cwd, out_tx, cmd_rx));

    let pump = tokio::spawn(async move {
        while let Some(bytes) = out_rx.recv().await {
            if sender.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = receiver.next().await {
        let cmd = match msg {
            Message::Binary(b) => Cmd::Input(b.to_vec()),
            Message::Text(t) => {
                // A resize arrives as `\x00cols,rows` — out of band, since everything else the
                // browser sends is keystrokes destined for the shell.
                match t.strip_prefix('\u{0}').and_then(|d| d.split_once(',')) {
                    Some((c, r)) => match (c.parse(), r.parse()) {
                        (Ok(cols), Ok(rows)) => Cmd::Resize(cols, rows),
                        _ => continue,
                    },
                    None => Cmd::Input(t.as_bytes().to_vec()),
                }
            }
            Message::Close(_) => break,
            _ => continue,
        };
        if cmd_tx.send(cmd).is_err() {
            break;
        }
    }

    drop(cmd_tx);
    pump.abort();
    let _ = spawned.join();
}

/// Own the pty for the life of one session.
fn pty_thread(
    cwd: &str,
    out: tokio::sync::mpsc::Sender<Vec<u8>>,
    cmds: std::sync::mpsc::Receiver<Cmd>,
) {
    let pty = NativePtySystem::default();
    let Ok(pair) = pty.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }) else {
        let _ = out.blocking_send(b"could not open a pty\r\n".to_vec());
        return;
    };

    let mut cmd = CommandBuilder::new(shell());
    cmd.cwd(cwd);
    // Tell the shell what it is talking to, so colour and line editing behave.
    cmd.env("TERM", "xterm-256color");

    let Ok(mut child) = pair.slave.spawn_command(cmd) else {
        let _ = out.blocking_send(b"could not start a shell\r\n".to_vec());
        return;
    };
    drop(pair.slave);

    let Ok(mut reader) = pair.master.try_clone_reader() else {
        return;
    };
    let mut writer = pair.master.take_writer().ok();

    // Reading is blocking and must not stall command handling.
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    while let Ok(cmd) = cmds.recv() {
        match cmd {
            Cmd::Input(bytes) => {
                if let Some(w) = writer.as_mut() {
                    let _ = std::io::Write::write_all(w, &bytes);
                    let _ = std::io::Write::flush(w);
                }
            }
            Cmd::Resize(cols, rows) => {
                let _ = pair.master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
        }
    }

    let _ = child.kill();
}
