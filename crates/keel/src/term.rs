//! A real terminal.
//!
//! Not a command runner: a PTY running the user's own shell. Anything less fails the moment you
//! need an interactive prompt, a pager, colour, or a long-running process — which is most of what a
//! terminal is for.
//!
//! The socket is bidirectional and byte-oriented. Terminal output is not line-structured, and
//! buffering it into lines would break every progress bar and prompt.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

/// The shell to run. Honour the user's own, since their prompt, aliases and PATH live there.
fn shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

pub async fn ws(
    ws: WebSocketUpgrade,
    crate::serve::Checkout(repo): crate::serve::Checkout,
) -> Response {
    ws.on_upgrade(move |socket| session(socket, repo.to_string()))
}

/// What the browser can ask the pty to do.
enum Cmd {
    Input(Vec<u8>),
    Resize(u16, u16),
}

/// What the pty sends back.
///
/// Two kinds on one socket, split the same way the browser's own messages are: bytes are output,
/// text is out of band. So a websocket text frame is the tab's title and never terminal output.
enum Out {
    Bytes(Vec<u8>),
    Title(String),
}

/// The program name to show on the tab, from whatever `ps` prints for a pid.
///
/// `comm` is a path on macOS (`/bin/zsh`) and a login shell wears a leading hyphen (`-zsh`), and a
/// tab labelled `/bin/zsh` is a tab labelled nothing.
fn program_name(raw: &str) -> Option<String> {
    let name = raw
        .trim()
        .rsplit('/')
        .next()?
        .trim_start_matches('-')
        .trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// What is running in the foreground of this pty right now — the shell when nothing else is.
fn foreground(pid: i32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    program_name(&String::from_utf8_lossy(&out.stdout))
}

async fn session(socket: WebSocket, cwd: String) {
    use futures_util::{SinkExt, StreamExt};
    let (mut sender, mut receiver) = socket.split();

    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Out>(256);
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Cmd>();

    // The pty master is Send but not Sync, so it cannot live in an async task that awaits. One
    // owning thread holds it, reads from it, and applies commands; the async side only moves bytes.
    let spawned = std::thread::spawn(move || pty_thread(&cwd, out_tx, cmd_rx));

    let pump = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let frame = match msg {
                Out::Bytes(bytes) => Message::Binary(bytes.into()),
                Out::Title(name) => Message::Text(name.into()),
            };
            if sender.send(frame).await.is_err() {
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
    out: tokio::sync::mpsc::Sender<Out>,
    cmds: std::sync::mpsc::Receiver<Cmd>,
) {
    let pty = NativePtySystem::default();
    let Ok(pair) = pty.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }) else {
        let _ = out.blocking_send(Out::Bytes(b"could not open a pty\r\n".to_vec()));
        return;
    };

    let mut cmd = CommandBuilder::new(shell());
    cmd.cwd(cwd);
    // Tell the shell what it is talking to, so colour and line editing behave.
    cmd.env("TERM", "xterm-256color");

    let Ok(mut child) = pair.slave.spawn_command(cmd) else {
        let _ = out.blocking_send(Out::Bytes(b"could not start a shell\r\n".to_vec()));
        return;
    };
    drop(pair.slave);

    let Ok(mut reader) = pair.master.try_clone_reader() else {
        return;
    };
    let titles = out.clone();
    let mut writer = pair.master.take_writer().ok();

    // Reading is blocking and must not stall command handling.
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out.blocking_send(Out::Bytes(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Whatever is in the pty's foreground process group is what the person is running, which is
    // the only honest name for the tab: `zsh` at a prompt, `cargo` while it builds. Polled on the
    // command loop's own timeout rather than from a third thread, and `ps` runs only when the
    // group actually changed — an idle terminal spawns nothing.
    // `pid_t` is `i32` everywhere this runs, and a whole dependency to spell that is not worth it.
    let mut showing: Option<i32> = None;
    let announce = |master: &(dyn portable_pty::MasterPty + Send), showing: &mut Option<i32>| {
        let pgid = master.process_group_leader();
        if pgid == *showing {
            return true;
        }
        *showing = pgid;
        match pgid.and_then(foreground) {
            Some(name) => titles.blocking_send(Out::Title(name)).is_ok(),
            None => true,
        }
    };
    announce(pair.master.as_ref(), &mut showing);

    loop {
        match cmds.recv_timeout(std::time::Duration::from_millis(700)) {
            Ok(Cmd::Input(bytes)) => {
                if let Some(w) = writer.as_mut() {
                    let _ = std::io::Write::write_all(w, &bytes);
                    let _ = std::io::Write::flush(w);
                }
            }
            Ok(Cmd::Resize(cols, rows)) => {
                let _ = pair.master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if !announce(pair.master.as_ref(), &mut showing) {
            break;
        }
    }

    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `ps -o comm=` actually prints, on both platforms and for a login shell.
    #[test]
    fn a_tab_is_named_after_the_program_not_its_path() {
        assert_eq!(program_name("/bin/zsh\n").as_deref(), Some("zsh"));
        assert_eq!(program_name("-zsh").as_deref(), Some("zsh"));
        assert_eq!(program_name("cargo\n").as_deref(), Some("cargo"));
        assert_eq!(program_name("/usr/bin/vim ").as_deref(), Some("vim"));
        // A dead pid prints nothing, and an empty tab title is worse than the number it replaces.
        assert_eq!(program_name(""), None);
        assert_eq!(program_name("  \n"), None);
    }
}
