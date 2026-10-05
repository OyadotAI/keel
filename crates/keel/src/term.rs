//! A real terminal.
//!
//! Not a command runner: a PTY running the user's own shell. Anything less fails the moment you
//! need an interactive prompt, a pager, colour, or a long-running process — which is most of what a
//! terminal is for.
//!
//! The socket is bidirectional and byte-oriented. Terminal output is not line-structured, and
//! buffering it into lines would break every progress bar and prompt.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

/// The shell to run. Honour the user's own, since their prompt, aliases and PATH live there.
#[cfg(unix)]
fn shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

/// `%COMSPEC%`, the shell Windows itself says is the command shell.
#[cfg(windows)]
fn shell() -> String {
    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
}

#[derive(serde::Deserialize)]
pub struct TermQuery {
    /// `claude` or `codex` to run the agent itself; absent for the person's shell.
    agent: Option<String>,
    /// The conversation: one Keel chose for a new session, or one to resume.
    session: Option<String>,
    #[serde(default)]
    resume: bool,
    lane: Option<String>,
    /// The person chose "Take over here": resume even though another process has it open.
    #[serde(default)]
    takeover: bool,
}

/// The close code that says "this conversation is open in another process": the page shows
/// that it is following it there, rather than an agent that exited. Private range (4000–4999).
pub const ELSEWHERE: u16 = 4001;

/// What the PTY runs.
enum Launch {
    Shell,
    Agent {
        agent: String,
        repo: camino::Utf8PathBuf,
        port: u16,
        session: String,
        resume: bool,
        lane: Option<String>,
    },
}

/// A session id goes on a command line: a UUID's characters and nothing else, so it can never be
/// read as a flag.
fn plausible_session(id: &str) -> bool {
    // At least one hex digit, and not starting with a dash: `-` alone, or `--x`, is a flag.
    id.len() >= 8
        && id.len() <= 64
        && !id.starts_with('-')
        && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

pub async fn ws(
    ws: WebSocketUpgrade,
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::serve::AppState>>,
    crate::serve::Checkout(checkout): crate::serve::Checkout,
    axum::extract::Query(q): axum::extract::Query<TermQuery>,
) -> Response {
    // no-blocking: a WebSocket upgrade; the PTY runs in its own task and threads.
    let launch = match (q.agent, q.session) {
        (Some(agent), Some(session)) if plausible_session(&session) => Launch::Agent {
            agent,
            repo: state.repo(),
            port: state.port(),
            session,
            resume: q.resume,
            lane: q.lane.filter(|l| !l.is_empty()),
        },
        (Some(_), _) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "an agent terminal needs a session id",
            )
                .into_response();
        }
        _ => Launch::Shell,
    };
    // A conversation another `claude` has open is two processes appending to one transcript,
    // whether this one resumes it or names it as new. Every way a lane starts passes here —
    // opened from History, restored at launch, started again — so this is where it is refused,
    // not in whichever of them remembered to ask.
    let elsewhere = match &launch {
        Launch::Agent { agent, session, .. } if agent == "claude" && !q.takeover => {
            Some(session.clone())
        }
        _ => None,
    };
    // The desktop app offers `keel` beside its token (`pair::app_token`); a browser closes a
    // socket whose server picked none of the protocols it offered.
    ws.protocols(["keel"])
        .on_upgrade(move |mut socket| async move {
            if let Some(id) = elsewhere {
                let open = tokio::task::spawn_blocking(move || {
                    keel_workspace::claude_home()
                        .is_some_and(|home| keel_workspace::status(&home, &id).is_some())
                })
                .await
                .unwrap_or(false);
                if open {
                    let _ = socket
                        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                            code: ELSEWHERE,
                            reason: "open in another process".into(),
                        })))
                        .await;
                    return;
                }
            }
            session(socket, checkout.to_string(), launch).await
        })
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

async fn session(socket: WebSocket, cwd: String, launch: Launch) {
    use futures_util::{SinkExt, StreamExt};
    let (mut sender, mut receiver) = socket.split();

    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Out>(256);
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Cmd>();

    // The pty master is Send but not Sync, so it cannot live in an async task that awaits. One
    // owning thread holds it, reads from it, and applies commands; the async side only moves bytes.
    let spawned = std::thread::spawn(move || pty_thread(&cwd, launch, out_tx, cmd_rx));

    let mut pump = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let frame = match msg {
                Out::Bytes(bytes) => Message::Binary(bytes.into()),
                Out::Title(name) => Message::Text(name.into()),
            };
            if sender.send(frame).await.is_err() {
                return;
            }
        }
        // The program ended — or never started — and its output is all sent. Closing the socket
        // is what tells the page so: without it a terminal whose agent had exited sat there
        // taking keystrokes into nothing, with no "exited" and no way to start it again.
        let _ = sender.send(Message::Close(None)).await;
    });

    loop {
        let msg = tokio::select! {
            m = receiver.next() => match m {
                Some(Ok(m)) => m,
                _ => break,
            },
            _ = &mut pump => break,
        };
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
    launch: Launch,
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

    let mut cmd = match launch {
        Launch::Shell => CommandBuilder::new(shell()),
        Launch::Agent {
            agent,
            repo,
            port,
            session,
            resume,
            lane,
        } => {
            // Two agents may share a tree: one reading or planning beside one editing is the
            // point of a shared lane. What must not happen is two *turns* writing it at once,
            // and that is held where it can be seen — a busy terminal session claims
            // `term:<session>` on its tree (non-negotiable 11), so the overlapping turn gets its
            // files noted and nothing committed. Refusing the second terminal outright blocked
            // every second lane in a project behind an idle prompt.
            let checkout = camino::Utf8PathBuf::from(cwd);
            match crate::agent::interactive(
                &agent,
                &repo,
                &checkout,
                port,
                &session,
                resume,
                lane.as_deref(),
            ) {
                Ok((program, args)) => {
                    let mut c = CommandBuilder::new(program);
                    c.args(args);
                    c
                }
                Err(why) => {
                    let _ = out.blocking_send(Out::Bytes(format!("{why}\r\n").into_bytes()));
                    return;
                }
            }
        }
    };
    cmd.cwd(cwd);
    // Not a child of whatever Claude Code session launched Keel — in the shell either, where the
    // person may well type `claude` themselves.
    for name in crate::agent::INHERITED {
        cmd.env_remove(name);
    }
    // Tell the shell what it is talking to, so colour and line editing behave.
    cmd.env("TERM", "xterm-256color");

    let Ok(mut child) = pair.slave.spawn_command(cmd) else {
        let _ = out.blocking_send(Out::Bytes(
            b"could not start it: is it installed and on PATH?\r\n".to_vec(),
        ));
        return;
    };
    // Taken now: the group outlives its leader, so ending it after an exit needs the number.
    let leader = child.process_id();
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
        // Windows' ConPTY has no foreground group to read; the tab keeps its plain name there.
        #[cfg(unix)]
        let pgid = master.process_group_leader();
        #[cfg(not(unix))]
        let pgid: Option<i32> = {
            let _ = master;
            None
        };
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
        // It exited on its own — `/exit`, ⌃C twice, a crash. Say so, and end the session.
        if let Ok(Some(status)) = child.try_wait() {
            let _ = titles.blocking_send(Out::Bytes(
                format!("\r\n[exited {}]\r\n", status.exit_code()).into_bytes(),
            ));
            break;
        }
        if !announce(pair.master.as_ref(), &mut showing) {
            break;
        }
    }

    // The agent's whole tree, not its top: `claude` with a `cargo test` under it is the ordinary
    // shape, and killing the leader alone leaves the rest running.
    // Everything under the agent, taken while it is still alive to be walked from: its tool
    // shells lead process groups of their own, and its group alone does not reach them.
    #[cfg(unix)]
    let stragglers = leader.map(crate::signals::descendants).unwrap_or_default();
    if matches!(child.try_wait(), Ok(None)) {
        if let Some(pid) = child.process_id() {
            // Asked first, briefly: an interrupted `claude` flushes its transcript and removes
            // its own pid file. Killed outright it leaves that file saying "busy", and a
            // session that reads as live is refused a resume and holds its tree.
            crate::signals::group(pid, crate::signals::INTERRUPT);
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
            while std::time::Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            crate::signals::end_tree(pid);
        }
        let _ = child.kill();
    }
    // Whatever the agent left in its group, however it ended. An agent that exits on its own —
    // `/exit`, a crash — can leave a `sleep &` or a `cargo test` behind in its process group,
    // reparented to init and running with nothing left that could stop it. The group outlives
    // its leader while anything in it is alive, so it is ended either way.
    if let Some(pid) = leader {
        #[cfg(unix)]
        crate::signals::end_all(pid, &stragglers);
        #[cfg(not(unix))]
        crate::signals::end_tree(pid);
    }
    // Reaped, always: a zombie answers `kill(pid, 0)`, so an unreaped agent read as a live
    // session for the rest of the daemon's life. Bounded — it has been sent SIGKILL.
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `ps -o comm=` actually prints, on both platforms and for a login shell.
    /// A session id goes on a command line, so nothing that reads as a flag gets there.
    #[test]
    fn a_session_id_cannot_be_a_flag() {
        assert!(plausible_session("6aa9ed86-7016-449f-958b-b65909ed4c3b"));
        assert!(!plausible_session("-"));
        assert!(!plausible_session("--------"));
        assert!(!plausible_session("--resume"));
        assert!(!plausible_session("../../etc"));
    }

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
