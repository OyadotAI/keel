//! Ten evals: the product's own promises, checked against a real agent.
//!
//!
//! Everything else Keel has is a unit test — 248 in Rust, 88 in Swift — and the app still shipped
//! a build that could not run a single `Bash` call. Every piece was correct; the composition was
//! not. `keel approve` was passed an argument it did not accept, which is a fact about two files
//! agreeing, and no test of either file could see it. These are the tests that can.
//!
//! Each one names a promise the product makes and checks it against the real thing — the daemon,
//! the hook, `claude`, the gate. Six spend tokens (a few cents in total) and are gated on
//! `KEEL_EVALS`; four cost nothing and run with `make check`.
//!
//! | # | Promise |
//! |---|---|
//! | 1 | A turn edits files and runs commands, and the work lands |
//! | 2 | A refused command comes back as a question, and answering releases the agent |
//! | 3 | Denying stops the turn rather than letting it substitute |
//! | 4 | Stop interrupts the agent *and* what it started |
//! | 5 | The gate's verdict is the exit code, so a failing check fails |
//! | 6 | A lane writes in its own checkout, never the project |
//! | 7 | A question reaches the lane that asked, and the answer returns as the tool result |
//! | 8 | A session resumes: the second turn remembers the first |
//! | 9 | Codex runs and streams |
//! | 10 | A slash command is Claude Code's to resolve, and costs nothing |

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Skip a paid eval unless the caller asked for it, saying so rather than passing quietly.
macro_rules! paid {
    () => {
        if std::env::var("KEEL_EVALS").is_err() {
            eprintln!("  skipped — set KEEL_EVALS to spend tokens (`make evals`)");
            return;
        }
    };
}

/// 1. The whole loop: the agent writes a file, runs a command, the project's gate accepts it.
///
/// The one that would have caught the build that refused every `Bash` call, and the one the
/// replay fixtures are recorded from.
#[test]
fn eval_01_a_turn_edits_and_runs_and_the_work_lands() {
    paid!();
    let (repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttouch gate-ran\n")]);
    let dir = repo.path().to_path_buf();

    let (stream, answered) = turn(
        port,
        "lane-1",
        "Create a file called hello.txt containing exactly: ok\n\
         Then run the shell command: echo done\n\
         Do nothing else and keep your reply to one sentence.",
    );
    let verdict = get(port, "/api/verify").unwrap_or_default();
    stop_daemon(&mut daemon);

    if let Ok(path) = std::env::var("KEEL_RECORD") {
        std::fs::write(&path, &stream).expect("recording");
    }

    assert!(
        stream.contains("event: msg"),
        "no output reached the client:\n{stream}"
    );
    assert!(
        !stream.contains("event: fatal"),
        "the agent would not start:\n{stream}"
    );
    assert!(
        answered > 0,
        "nothing was asked — the hook never reached the daemon"
    );
    assert!(
        dir.join("hello.txt").exists(),
        "the file was never written:\n{stream}"
    );
    assert!(
        dir.join("gate-ran").exists(),
        "the gate never ran: {verdict}"
    );
    assert!(
        verdict.contains("event: done\ndata: 0"),
        "the gate failed:\n{verdict}"
    );
}

/// 2. A refused command comes back as a question, and answering releases the agent to run that
///    same command rather than a substitute for it.
#[test]
fn eval_02_a_refusal_becomes_a_question_and_the_answer_releases_it() {
    paid!();
    let (repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);
    let dir = repo.path().to_path_buf();

    let (stream, answered) = turn(
        port,
        "lane-2",
        "Run exactly this shell command and nothing else: touch approved-me\n\
         Reply with one word when it succeeds.",
    );
    stop_daemon(&mut daemon);

    assert!(
        answered > 0,
        "the command ran without anybody being asked:\n{stream}"
    );
    assert!(
        dir.join("approved-me").exists(),
        "the approval was answered and the command still never ran — the agent was not released"
    );
}

/// 3. Denying stops the turn rather than letting the agent find another way. This is the reason
///    approvals block at all: refused, it used to work around the gap and carry on.
#[test]
fn eval_03_denying_stops_the_turn_rather_than_substituting() {
    paid!();
    let (repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);
    let dir = repo.path().to_path_buf();

    let done = flag();
    let d = done.clone();
    let runner = std::thread::spawn(move || {
        let out = chat(
            port,
            "lane-3",
            "Run exactly this shell command: touch denied-me\n\
             If you cannot run it, say so and stop. Do not use another way to make the file.",
        );
        d.store(true, std::sync::atomic::Ordering::SeqCst);
        out
    });
    let denied = respond(port, "lane-3", &done, "deny");
    let stream = runner.join().expect("turn thread");
    stop_daemon(&mut daemon);

    assert!(
        denied > 0,
        "nothing was asked, so nothing was denied:\n{stream}"
    );
    assert!(
        !dir.join("denied-me").exists(),
        "denied, and the file exists anyway — the agent found another way, which is exactly what \
         blocking approvals exist to prevent"
    );
}

/// 4. Stop interrupts the agent *and* the process it started. `CLAUDE.md` calls this
///    non-negotiable #6 and it was untrue: Stop closed the stream and the agent kept running.
#[test]
fn eval_04_stop_interrupts_the_agent_and_what_it_started() {
    paid!();
    let (repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);
    let marker = repo.path().join("still-running");

    let done = flag();
    let d = done.clone();
    let runner = std::thread::spawn(move || {
        let out = chat(
            port,
            "lane-4",
            "Run exactly this shell command in the foreground and wait for it to finish:\n\
             sh -c 'sleep 25; touch still-running'\n\
             Say nothing until it finishes.",
        );
        d.store(true, std::sync::atomic::Ordering::SeqCst);
        out
    });

    // Answering runs in the background: `respond_for` returns only when the turn is *over*, so
    // waiting for it and then calling Stop is calling Stop on nothing — which is what this test
    // did on its first run, and reported as a product failure.
    let granted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = granted.clone();
    let d2 = done.clone();
    let answering = std::thread::spawn(move || {
        respond_counting(
            port,
            "lane-4",
            &d2,
            "allow",
            Duration::from_secs(90),
            &counter,
        )
    });

    // Stop once the command is actually running, not before it has been approved. Waiting on the
    // answering thread's own count rather than polling: polling would take the question away from
    // it and the agent would never be released at all.
    let waited = Instant::now();
    while granted.load(std::sync::atomic::Ordering::SeqCst) == 0
        && !done.load(std::sync::atomic::Ordering::SeqCst)
        && waited.elapsed() < Duration::from_secs(90)
    {
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_secs(6));
    let stopped = post_read(port, "/api/chat/stop?lane=lane-4", "{}").unwrap_or_default();
    let _ = runner.join();
    let answered = answering.join().unwrap_or(0);

    // Well past the sleep: a child that survived the interrupt leaves the marker in this window.
    std::thread::sleep(Duration::from_secs(30));
    let survived = marker.exists();
    stop_daemon(&mut daemon);

    assert!(
        answered > 0,
        "the command was never approved, so nothing was running to stop"
    );
    assert!(
        stopped.contains("\"stopped\":true"),
        "stop found nothing to signal: {stopped}"
    );
    assert!(
        !survived,
        "the command the agent started outlived Stop — the interrupt never reached its group"
    );
}

/// 5. The gate's verdict is the exit code, not a summary. Free: no agent involved.
#[test]
fn eval_05_a_failing_gate_reports_failure() {
    let (_pass, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);
    let good = get(port, "/api/verify").unwrap_or_default();
    stop_daemon(&mut daemon);

    let (_fail, port, mut daemon) = project(&[("Makefile", "check:\n\texit 3\n")]);
    let bad = get(port, "/api/verify").unwrap_or_default();
    stop_daemon(&mut daemon);

    assert!(
        good.contains("event: done\ndata: 0"),
        "a passing check did not report 0:\n{good}"
    );
    // Non-zero, not a specific number: `make` exits 2 when a recipe fails, and the recipe's own
    // 3 never reaches the caller. What Keel promises is that the code decides, not a summary.
    let code = bad
        .rsplit("event: done\ndata: ")
        .next()
        .unwrap_or("")
        .trim();
    assert!(
        !code.is_empty() && code != "0",
        "a check that exited non-zero was not reported as a failure — a gate that cannot fail is \
         not a gate:\n{bad}"
    );
}

/// 6. A lane resolves to its own checkout, and a lane with no checkout is refused rather than
///    quietly served the project. Free: this is the isolation contract, not the agent.
#[test]
fn eval_06_a_lane_resolves_to_its_own_checkout() {
    let (repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);
    let dir = repo.path().to_path_buf();

    let made =
        post_read(port, "/api/worktree/create", r#"{"name":"eval-lane"}"#).unwrap_or_default();
    let tree = get(port, "/api/tree?wt=eval-lane").unwrap_or_default();
    let stray = get(port, "/api/tree?wt=not-a-lane");
    stop_daemon(&mut daemon);

    assert!(
        dir.join(".keel/worktrees/eval-lane").is_dir(),
        "the lane got no checkout of its own: {made}"
    );
    assert!(
        !tree.is_empty(),
        "the lane's own checkout could not be read: {tree}"
    );
    assert!(
        stray.is_none(),
        "a lane with no checkout was served the project instead of being refused — isolation that \
         falls back is not isolation"
    );
}

/// 7. A question reaches the lane that asked it, and the answer comes back as the tool's result.
#[test]
fn eval_07_a_question_reaches_its_lane_and_the_answer_returns() {
    paid!();
    let (_repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);

    let done = flag();
    let d = done.clone();
    let runner = std::thread::spawn(move || {
        let out = chat(
            port,
            "lane-7",
            "Use the `ask_user` tool to ask me: Which colour? with options Red and Blue.\n\
             Then reply with exactly the colour I chose and nothing else.",
        );
        d.store(true, std::sync::atomic::Ordering::SeqCst);
        out
    });
    let answered = answer_question(port, "lane-7", &done, "Blue");
    let stream = runner.join().expect("turn thread");
    stop_daemon(&mut daemon);

    assert!(
        answered > 0,
        "the question never reached the queue:\n{stream}"
    );
    assert!(
        stream.contains("Blue"),
        "the answer never came back to the agent as the tool's result:\n{stream}"
    );
}

/// 8. A session resumes: the second turn is the same conversation and remembers the first.
#[test]
fn eval_08_a_session_resumes_and_remembers() {
    paid!();
    let (_repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);

    let (first, _) = turn(
        port,
        "lane-8",
        "Remember the word GRAPEFRUIT. Reply with just: ok",
    );
    let session =
        field(&first, "session_id").unwrap_or_else(|| panic!("no session id in:\n{first}"));

    let url = format!(
        "http://127.0.0.1:{port}/api/chat?provider=claude&mode=plan&lane=lane-8&session={session}&prompt={}",
        urlencode("What word did I ask you to remember? Reply with just that word.")
    );
    let second = curl_stream(&url);
    stop_daemon(&mut daemon);

    assert!(
        second.to_uppercase().contains("GRAPEFRUIT"),
        "the resumed turn did not remember the first — the session was not continued:\n{second}"
    );
}

/// 9. Codex runs and its output reaches the client. It shipped producing nothing at all, because
///    the branch that built its command never piped stdout.
#[test]
fn eval_09_codex_runs_and_streams() {
    paid!();
    if which("codex").is_none() {
        eprintln!("  skipped — codex is not installed");
        return;
    }
    let (_repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);
    let url = format!(
        "http://127.0.0.1:{port}/api/chat?provider=codex&mode=plan&lane=lane-9&prompt={}",
        urlencode("Reply with exactly the word OK and nothing else.")
    );
    let stream = curl_stream(&url);
    stop_daemon(&mut daemon);

    assert!(
        !stream.contains("event: fatal"),
        "codex would not start:\n{stream}"
    );
    assert!(
        stream.contains("event: msg"),
        "codex produced no output the client could see — the exact shape of it shipping dead:\n{stream}"
    );
    assert!(
        stream.contains("turn.completed") || stream.contains("agent_message"),
        "codex started but never finished a turn:\n{stream}"
    );
}

/// 10. A slash command belongs to Claude Code, which resolves it locally for no tokens and no
///     turn. That is why Keel sends them verbatim rather than implementing any of them — and why
///     the `/` picker can offer whatever `claude` reports without checking it first.
#[test]
fn eval_10_a_slash_command_is_resolved_by_claude_for_nothing() {
    paid!();
    let (_repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);
    let url = format!(
        "http://127.0.0.1:{port}/api/chat?provider=claude&mode=plan&lane=lane-10&prompt={}",
        urlencode("/compact")
    );
    let stream = curl_stream(&url);
    stop_daemon(&mut daemon);

    assert!(
        stream.contains("\"slash_commands\""),
        "the init record carried no command list — the `/` picker has nothing to offer:\n{stream}"
    );
    assert!(
        stream.contains("\"num_turns\":0"),
        "a slash command cost a turn, so it went to the model as text rather than being resolved"
    );
}

// ── the harness ──────────────────────────────────────────────────────────────────────────────

/// A git repository with a daemon serving it. Every eval starts here.
fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, u16, Child) {
    let repo = tempfile::tempdir().expect("tempdir");
    let dir = repo.path();
    for (name, body) in files {
        std::fs::write(dir.join(name), body).expect("write");
    }
    std::fs::write(dir.join("README.md"), "eval\n").expect("README");
    for args in [
        vec!["init", "-q"],
        vec!["add", "-A"],
        vec![
            "-c",
            "user.email=e@k",
            "-c",
            "user.name=Evals",
            "commit",
            "-qm",
            "start",
        ],
    ] {
        assert!(
            Command::new("git")
                .args(&args)
                .current_dir(dir)
                .status()
                .expect("git")
                .success(),
            "git {args:?} failed"
        );
    }
    let port = free_port();
    let daemon = serve(dir, port);
    wait_for_daemon(port);
    (repo, port, daemon)
}

fn stop_daemon(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn flag() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
}

/// Run one turn, answering every approval as a person at the keyboard would.
///
/// The two overlap because the agent is genuinely blocked until something answers — that is what
/// the hook is for, and a test that answered afterwards would be testing nothing.
fn turn(port: u16, lane: &str, prompt: &str) -> (String, usize) {
    let done = flag();
    let d = done.clone();
    let lane_owned = lane.to_string();
    let prompt_owned = prompt.to_string();
    let runner = std::thread::spawn(move || {
        let out = chat(port, &lane_owned, &prompt_owned);
        d.store(true, std::sync::atomic::Ordering::SeqCst);
        out
    });
    let answered = respond(port, lane, &done, "allow");
    (runner.join().expect("turn thread"), answered)
}

fn respond(port: u16, lane: &str, done: &std::sync::atomic::AtomicBool, decision: &str) -> usize {
    respond_for(port, lane, done, decision, Duration::from_secs(300))
}

/// Answer approvals until the turn ends or time runs out.
fn respond_for(
    port: u16,
    lane: &str,
    done: &std::sync::atomic::AtomicBool,
    decision: &str,
    limit: Duration,
) -> usize {
    respond_counting(
        port,
        lane,
        done,
        decision,
        limit,
        &std::sync::atomic::AtomicUsize::new(0),
    )
}

/// The same, reporting each answer as it is given.
///
/// Needed because `/api/approve/poll` *drains* the queue: a second reader looking to see whether
/// anything is pending takes the question away from the thread that would have answered it, and
/// the agent waits out the full timeout. Only one reader, and it says what it has done.
fn respond_counting(
    port: u16,
    lane: &str,
    done: &std::sync::atomic::AtomicBool,
    decision: &str,
    limit: Duration,
    seen: &std::sync::atomic::AtomicUsize,
) -> usize {
    let deadline = Instant::now() + limit;
    let mut answered = 0;
    while Instant::now() < deadline && !done.load(std::sync::atomic::Ordering::SeqCst) {
        if let Some(body) = get(port, &format!("/api/approve/poll?lane={lane}")) {
            for id in ids(&body) {
                post(
                    port,
                    "/api/approve/answer",
                    &format!(
                        r#"{{"id":"{id}","decision":"{decision}","scope":"session","rules":[]}}"#
                    ),
                );
                answered += 1;
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    answered
}

/// Answer an `ask_user` question with a choice, which travels back as the tool's result.
fn answer_question(
    port: u16,
    lane: &str,
    done: &std::sync::atomic::AtomicBool,
    choice: &str,
) -> usize {
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut answered = 0;
    while Instant::now() < deadline && !done.load(std::sync::atomic::Ordering::SeqCst) {
        if let Some(body) = get(port, &format!("/api/approve/poll?lane={lane}")) {
            for id in ids(&body) {
                post(
                    port,
                    "/api/approve/answer",
                    &format!(
                        r#"{{"id":"{id}","decision":"deny","scope":"session","rules":[],"answer":"{choice}"}}"#
                    ),
                );
                answered += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    answered
}

/// The first value of a JSON string field anywhere in the stream.
fn field(stream: &str, name: &str) -> Option<String> {
    let needle = format!("\"{name}\":\"");
    let at = stream.find(&needle)? + needle.len();
    let rest = &stream[at..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn which(program: &str) -> Option<String> {
    let out = Command::new("sh")
        .args(["-lc", &format!("command -v {program}")])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn curl_stream(url: &str) -> String {
    let out = Command::new("curl")
        .args(["-sN", "--max-time", "300", url])
        .output()
        .expect("curl");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn post_read(port: u16, path: &str, body: &str) -> Option<String> {
    let mut child = Command::new("curl")
        .args([
            "-s",
            "--max-time",
            "20",
            "-X",
            "POST",
            "-H",
            "content-type: application/json",
            "-d",
            "@-",
            &format!("http://127.0.0.1:{port}{path}"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let _ = child.stdin.take()?.write_all(body.as_bytes());
    let out = child.wait_with_output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    port
}

fn serve(dir: &std::path::Path, port: u16) -> Child {
    Command::new(env!("CARGO_BIN_EXE_keel"))
        .arg("serve")
        .arg(dir)
        .args(["--port", &port.to_string(), "--no-open"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("could not start the daemon")
}

fn wait_for_daemon(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if get(port, "/api/state").is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("the daemon never answered on {port}");
}

/// Stream a turn, returning everything the daemon sent.
fn chat(port: u16, lane: &str, prompt: &str) -> String {
    let url = format!(
        "http://127.0.0.1:{port}/api/chat?provider=claude&mode=acceptEdits&lane={lane}&prompt={}",
        urlencode(prompt)
    );
    let out = Command::new("curl")
        .args(["-sN", "--max-time", "300", &url])
        .output()
        .expect("curl");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The `id` of every object in a small JSON array, without a parser.
fn ids(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(at) = rest.find("\"id\":\"") {
        let after = &rest[at + 6..];
        let Some(end) = after.find('"') else { break };
        out.push(after[..end].to_string());
        rest = &after[end..];
    }
    out
}

fn get(port: u16, path: &str) -> Option<String> {
    let out = Command::new("curl")
        .args([
            "-sf",
            "--max-time",
            "10",
            &format!("http://127.0.0.1:{port}{path}"),
        ])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn post(port: u16, path: &str, body: &str) {
    let mut child = Command::new("curl")
        .args([
            "-sf",
            "--max-time",
            "10",
            "-X",
            "POST",
            "-H",
            "content-type: application/json",
            "-d",
            "@-",
            &format!("http://127.0.0.1:{port}{path}"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("curl");
    let _ = child
        .stdin
        .take()
        .expect("stdin")
        .write_all(body.as_bytes());
    let _ = child.wait();
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
