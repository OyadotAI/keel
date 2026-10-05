//! The daemon dies with the app — the budget that mattered most in the Swift suite, kept when the
//! Swift app went.
//!
//! The desktop app holds the write end of the daemon's stdin and nothing else ties them together:
//! macOS has no `PR_SET_PDEATHSIG`, Windows has no parent to poll, and a crash or a force quit
//! runs no cleanup at all. So the one thing that must hold is that closing that pipe, however it
//! closes, ends the daemon. A daemon that outlived its app reparented to init and kept serving,
//! one more invisible agent host per launch. Runs on every platform: there is no shell here.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .expect("a free port")
}

#[test]
fn the_daemon_exits_when_the_app_lets_go_of_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_keel"))
        .args([
            "serve",
            dir.path().to_str().unwrap(),
            "--port",
            &port.to_string(),
            "--exit-on-stdin-eof",
        ])
        .env("KEEL_PERMISSIONS_DIR", dir.path().join("permissions"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the daemon starts");
    let mut tie = child.stdin.take().expect("a stdin to hold");
    writeln!(tie, "{}", "t".repeat(64)).expect("the token goes over");

    // Up and serving while the pipe is held.
    let deadline = Instant::now() + Duration::from_secs(20);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < deadline,
            "the daemon never started listening"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "the daemon exited while its app was alive"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // The app goes — however it goes, this is what the daemon sees.
    drop(tie);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            return;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the daemon outlived its app by ten seconds");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
