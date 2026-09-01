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

// ── the second ten: what Keel promises about itself ──────────────────────────────────────────
//
// `CLAUDE.md` lists eleven non-negotiables and says they are enforced by tests. #6 — "Stop sends
// SIGINT" — was false for months, so the claim was worth checking. Four more of them had no test
// that ran the real thing, and they are here.
//
// All ten cost nothing. Three of them run a *stubbed* agent, which is how the command line Keel
// actually builds becomes observable: `keel-harness` has a unit test that `--bare` is never
// passed, but the daemon builds its own command in `api.rs` and nothing checked that one.

/// A fake `claude` on the daemon's PATH that records its arguments and emits a valid stream.
///
/// The daemon takes its PATH from the login shell (`$SHELL -lic 'printf %s "$PATH"'`), so
/// pointing `SHELL` at a script that prints the directory we want puts the stub first. Nothing
/// else can: an inherited `PATH` is appended *after* the shell's, behind the real `claude`.
fn stub_agent(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let bin = dir.join("stub-bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let argv = dir.join("argv.txt");

    let claude = bin.join("claude");
    std::fs::write(
        &claude,
        format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> {argv}\n\
             printf '%s\\n' '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"stub-1\",\"slash_commands\":[\"compact\"]}}'\n\
             printf '%s\\n' '{{\"type\":\"result\",\"subtype\":\"success\",\"session_id\":\"stub-1\",\"num_turns\":1,\"total_cost_usd\":0}}'\n",
            argv = argv.display()
        ),
    )
    .expect("stub");
    chmod_x(&claude);

    let shell = dir.join("stub-shell");
    std::fs::write(
        &shell,
        format!("#!/bin/sh\nprintf %s \"{}:$PATH\"\n", bin.display()),
    )
    .expect("shell");
    chmod_x(&shell);
    (shell, argv)
}

fn chmod_x(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).expect("stat").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod");
}

/// A project whose daemon runs the stub agent instead of the real one.
fn stubbed_project() -> (tempfile::TempDir, u16, Child, std::path::PathBuf) {
    let repo = tempfile::tempdir().expect("tempdir");
    let dir = repo.path();
    std::fs::write(dir.join("Makefile"), "check:\n\ttrue\n").expect("Makefile");
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
                .success()
        );
    }
    let (shell, argv) = stub_agent(dir);
    let port = free_port();
    let daemon = Command::new(env!("CARGO_BIN_EXE_keel"))
        .arg("serve")
        .arg(dir)
        .args(["--port", &port.to_string(), "--no-open"])
        .env("SHELL", &shell)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("daemon");
    wait_for_daemon(port);
    (repo, port, daemon, argv)
}

/// Everything the stub was invoked with.
fn argv_of(path: &std::path::Path) -> String {
    for _ in 0..40 {
        if let Ok(s) = std::fs::read_to_string(path)
            && !s.trim().is_empty()
        {
            return s;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    String::new()
}

/// 11. Non-negotiable #2: `--bare` is never passed, because bare mode never reads the OAuth
///     credentials a subscription depends on — and the flags that make the session safe are.
///
/// `keel-harness` asserts this about the invocation *it* builds. The daemon builds its own, and
/// that is the one that runs.
#[test]
fn eval_11_the_command_line_keel_actually_runs_is_the_documented_one() {
    let (_repo, port, mut daemon, argv) = stubbed_project();
    let _ = curl_stream(&format!(
        "http://127.0.0.1:{port}/api/chat?provider=claude&mode=plan&lane=l&prompt={}",
        urlencode("hello")
    ));
    let line = argv_of(&argv);
    stop_daemon(&mut daemon);

    assert!(!line.is_empty(), "the agent was never invoked");
    assert!(
        !line.contains("--bare"),
        "`--bare` was passed, which never reads OAuth credentials — every subscription session \
         would fail to authenticate:\n{line}"
    );
    for flag in [
        "--output-format",
        "stream-json",
        "--settings",
        "--mcp-config",
    ] {
        assert!(
            line.contains(flag),
            "`{flag}` is missing from the command line:\n{line}"
        );
    }
}

/// 12. The mode on the command line is the mode the lane is in. `plan` explores and changes
///     nothing; sending it as `acceptEdits` would let a planning turn write to the repository.
#[test]
fn eval_12_the_permission_mode_matches_the_lane() {
    let (_repo, port, mut daemon, argv) = stubbed_project();
    for mode in ["plan", "acceptEdits"] {
        let _ = curl_stream(&format!(
            "http://127.0.0.1:{port}/api/chat?provider=claude&mode={mode}&lane=l&prompt={}",
            urlencode("hello")
        ));
    }
    let line = argv_of(&argv);
    stop_daemon(&mut daemon);

    assert!(
        line.contains("--permission-mode plan"),
        "a plan turn did not run in plan mode:\n{line}"
    );
    assert!(
        line.contains("--permission-mode acceptEdits"),
        "an edit turn did not run in acceptEdits mode:\n{line}"
    );
}

/// 13. A resumed turn continues a conversation rather than starting one. Without `--resume` the
///     agent has no memory of the session and the transcript forks.
#[test]
fn eval_13_a_resumed_turn_passes_the_session_it_resumes() {
    let (_repo, port, mut daemon, argv) = stubbed_project();
    let _ = curl_stream(&format!(
        "http://127.0.0.1:{port}/api/chat?provider=claude&mode=plan&lane=l&session=abc-123&prompt={}",
        urlencode("hello")
    ));
    let line = argv_of(&argv);
    stop_daemon(&mut daemon);

    assert!(
        line.contains("--resume abc-123"),
        "the session was not resumed, so the turn forked a new conversation:\n{line}"
    );
}

/// 14. Non-negotiable #7: listing sessions never shows what was said.
///
/// The switcher polls this constantly. Reading a transcript to render a list is not licence to
/// display it, and a leak here puts one project's conversation in another's window.
#[test]
fn eval_14_listing_sessions_never_returns_what_was_said() {
    let (repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);
    let secret = "PINEAPPLE-QUADRANT-77";

    // A transcript in the shape Claude Code writes, carrying something unmistakable.
    let home = repo.path().join("fake-claude");
    let key = crate_project_key(repo.path());
    let dir = home.join("projects").join(&key);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("sess-1.jsonl"),
        format!(
            "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"{secret}\"}}}}\n"
        ),
    )
    .expect("transcript");

    let listing = get(port, "/api/state").unwrap_or_default();
    stop_daemon(&mut daemon);

    assert!(
        !listing.contains(secret),
        "a session listing carried the text of a message — the switcher polls this constantly:\n\
         {listing}"
    );
}

/// Claude Code's own directory name for a project: the path with separators flattened.
fn crate_project_key(path: &std::path::Path) -> String {
    path.to_string_lossy().replace(['/', '.'], "-")
}

/// 15. Non-negotiable #8: a question belongs to one conversation.
///
/// Windows are per-session. The old `mem::take` meant whichever polled first swallowed every
/// window's questions and the others timed out into a refusal nobody saw.
#[test]
fn eval_15_a_question_belongs_to_one_conversation() {
    let (_repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);

    for lane in ["alpha", "beta"] {
        let l = lane.to_string();
        std::thread::spawn(move || {
            post(
                port,
                "/api/approve/ask",
                &format!(
                    r#"{{"tool_name":"Bash","tool_input":{{"command":"echo {l}"}},"session_id":"","lane":"{l}","tool_use_id":"t-{l}"}}"#
                ),
            );
        });
    }
    std::thread::sleep(Duration::from_secs(2));

    let mine = get(port, "/api/approve/poll?lane=alpha").unwrap_or_default();
    let theirs = get(port, "/api/approve/poll?lane=beta").unwrap_or_default();
    stop_daemon(&mut daemon);

    assert!(
        mine.contains("alpha"),
        "alpha's own question did not reach it: {mine}"
    );
    assert!(
        !mine.contains("beta"),
        "alpha was handed beta's question — one window swallowing another's is how they used to \
         time out into refusals nobody saw: {mine}"
    );
    assert!(
        theirs.contains("beta"),
        "beta's question was taken by alpha: {theirs}"
    );
}

/// 16. Non-negotiable #9: "allow once, this session" means *that* session.
#[test]
fn eval_16_a_session_rule_does_not_leak_to_another_conversation() {
    let (_repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);

    post(
        port,
        "/api/permissions/add",
        r#"{"rule":"Bash(docker *)","scope":"session","session":"session-A"}"#,
    );
    let a = get(port, "/api/permissions?session=session-A").unwrap_or_default();
    let b = get(port, "/api/permissions?session=session-B").unwrap_or_default();
    let none = get(port, "/api/permissions").unwrap_or_default();
    stop_daemon(&mut daemon);

    assert!(
        a.contains("docker"),
        "the rule did not apply to the session that made it: {a}"
    );
    assert!(
        !b.contains("docker"),
        "another conversation inherited a rule scoped to one — \"once, this session\" has to mean \
         that session: {b}"
    );
    assert!(
        !none.contains("docker"),
        "a session rule became a project rule: {none}"
    );
}

/// 17. Non-negotiable #10: Keel refuses to leave loopback until something is paired.
///
/// The check is at the bind, not in the settings UI, so a hand-edited `state.json` cannot open a
/// port either.
#[test]
fn eval_17_nothing_is_served_off_loopback_until_a_device_is_paired() {
    let (_repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);
    let lan = lan_address();
    let reachable = lan.is_some_and(|ip| {
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::new(ip, port),
            Duration::from_millis(500),
        )
        .is_ok()
    });
    let loopback = get(port, "/api/state").is_some();
    stop_daemon(&mut daemon);

    assert!(loopback, "the daemon was not reachable on loopback at all");
    assert!(
        !reachable,
        "the daemon answered on this machine's network address with nothing paired — the whole \
         repository, its sessions and its credentials, to anyone on the network"
    );
}

/// This machine's first non-loopback IPv4 address, if it has one.
fn lan_address() -> Option<std::net::IpAddr> {
    let out = Command::new("sh")
        .args(["-lc", "ipconfig getifaddr en0 || ipconfig getifaddr en1"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    text.parse().ok()
}

/// 18. The one hook Keel ships: a repository's own `.claude/settings.json` is moved aside before
///     any agent runs.
///
/// `--bare` is never passed, because bare mode never reads the OAuth credentials a subscription
/// depends on — and the price is that the repository's own settings load. A hook there is a shell
/// command that runs on the machine of whoever opens the repo.
///
/// `keel-harness::quarantine` existed for exactly this and was wired only to the `keel trust`
/// subcommand, which the application never runs. Opening somebody else's repository and taking one
/// turn executed their `SessionStart` hook with no prompt — verified by running a real turn
/// against a repo whose hook touched a file, and finding the file. This is that test.
#[test]
fn eval_18_a_repositorys_own_hooks_are_quarantined_before_the_agent_runs() {
    let (repo, port, mut daemon, argv) = stubbed_project_with(&[
        (
            ".claude/settings.json",
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
        ),
        (
            ".mcp.json",
            r#"{"mcpServers":{"theirs":{"command":"true"}}}"#,
        ),
    ]);
    let dir = repo.path().to_path_buf();

    let _ = curl_stream(&format!(
        "http://127.0.0.1:{port}/api/chat?provider=claude&mode=plan&lane=l&prompt={}",
        urlencode("hello")
    ));
    let _ = argv_of(&argv);
    stop_daemon(&mut daemon);

    for rel in [".claude/settings.json", ".mcp.json"] {
        assert!(
            !dir.join(rel).exists(),
            "`{rel}` was still in place when the agent started — it is a shell command that runs \
             on the machine of whoever opens the repository"
        );
        assert!(
            dir.join(".keel/quarantine").join(rel).exists(),
            "`{rel}` was removed rather than quarantined — it is the person's own repository \
             content and they have to be able to read it"
        );
    }
}

/// 19. Every turn is preceded by a snapshot, so it can be rewound.
///
/// A git tree from a throwaway index and no refs: the repository's own history is untouched by
/// something whose whole job is to be undone. The daemon takes it itself, before the agent is
/// spawned, and says so on the stream as the turn's first fact — the app used to ask for it
/// through an endpoint of its own, which meant a turn Keel did not drive never had one.
#[test]
fn eval_19_a_turn_can_be_rewound_to_the_tree_before_it() {
    let (repo, port, mut daemon, _argv) = stubbed_project();
    let dir = repo.path().to_path_buf();
    std::fs::write(dir.join("keep.txt"), "before\n").expect("write");

    let stream = curl_stream(&format!(
        "http://127.0.0.1:{port}/api/chat?provider=claude&mode=plan&lane=l&prompt={}",
        urlencode("hello")
    ));
    let tree = stream
        .lines()
        .filter(|l| l.contains(r#""kind":"turn.started""#))
        .find_map(|l| field(l, "snapshot"))
        .unwrap_or_default();

    std::fs::write(dir.join("keep.txt"), "after\n").expect("write");
    std::fs::write(dir.join("stray.txt"), "new\n").expect("write");

    let back =
        post_read(port, "/api/git/restore", &format!(r#"{{"tree":"{tree}"}}"#)).unwrap_or_default();
    stop_daemon(&mut daemon);

    assert!(
        tree.len() == 40,
        "no snapshot was taken before the turn, so it could never be undone: {stream}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("keep.txt")).unwrap_or_default(),
        "before\n",
        "the file was not put back: {back}"
    );
}

/// 20. Trust stops at the edge of the project it was granted for.
///
/// "Trust this project" is stored in one repository's own `.keel/permissions.json`, and that
/// scoping is why it is safe to offer. The trust check used to run *before* the "does this edit
/// leave the repository" check, so on a trusted project an edit to `/tmp`, to the home directory
/// or to another checkout was allowed without a question.
#[test]
fn eval_20_trust_does_not_cover_an_edit_that_leaves_the_project() {
    let (repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);
    post(port, "/api/permissions/trust", r#"{"trusted":true}"#);

    let inside = repo.path().join("inside.txt");
    std::thread::spawn(move || {
        post(
            port,
            "/api/approve/ask",
            &format!(
                r#"{{"tool_name":"Write","tool_input":{{"file_path":"{}"}},"session_id":"","lane":"t","tool_use_id":"w-out"}}"#,
                "/tmp/keel-eval-outside.txt"
            ),
        );
    });
    std::thread::sleep(Duration::from_secs(2));
    let queued = get(port, "/api/approve/poll?lane=t").unwrap_or_default();

    // And an edit that stays inside is not asked about, or trust would be worth nothing.
    let body = format!(
        r#"{{"tool_name":"Write","tool_input":{{"file_path":"{}"}},"session_id":"","lane":"t2","tool_use_id":"w-in"}}"#,
        inside.display()
    );
    let answered = post_read(port, "/api/approve/ask", &body).unwrap_or_default();
    stop_daemon(&mut daemon);

    assert!(
        queued.contains("w-out"),
        "a trusted project let an edit to /tmp through without a question — trust is scoped to one \
         repository and this is not in it: {queued}"
    );
    assert!(
        answered.contains("defer"),
        "an edit inside a trusted project was still asked about, which is the toll trust exists to \
         remove: {answered}"
    );
}

/// 21. A command the agent backgrounds outlives the turn, because Keel is the one running it.
///
/// Measured before this existed: a turn is one `claude -p`, and the CLI kills every tracked
/// background shell at teardown — `gh run watch` was `[killed]` eight seconds after the turn
/// ended, and the person found out six minutes later from a notification that only arrived
/// because they typed again. So the hook takes the call: Keel runs it, and the agent is refused
/// with the job's name rather than left holding a shell that is about to die.
///
/// Nobody is asked any more, and that is asserted here rather than assumed. "Should this keep
/// running after the turn?" was a card with no second answer worth having — "no" hands a dev
/// server back to a foreground `Bash` timeout that kills it having produced nothing — and a card
/// nobody was at the keyboard for cost four minutes before the same outcome. The job is listed
/// in Monitors while it runs, with its output and a Stop button, which is where being asked was
/// supposed to lead.
#[test]
fn eval_21_a_monitored_command_outlives_the_turn_that_asked_for_it() {
    let (_repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);

    let decision = post_read(
        port,
        "/api/approve/ask",
        r#"{"tool_name":"Bash","tool_input":{"command":"echo watching; sleep 1; echo done","run_in_background":true},"session_id":"","lane":"m","tool_use_id":"bg-1"}"#,
    )
    .unwrap_or_default();
    // Whatever was queued in the meantime. Nothing should have been.
    let queued = get(port, "/api/approve/poll?lane=m").unwrap_or_default();

    // Long past the point the agent's own shell would have been killed with the turn.
    std::thread::sleep(Duration::from_secs(3));
    let jobs = get(port, "/api/monitors?lane=m").unwrap_or_default();
    let other = get(port, "/api/monitors?lane=elsewhere").unwrap_or_default();
    stop_daemon(&mut daemon);

    assert!(
        decision.contains("deny") && decision.contains("background job"),
        "the agent was not told which job its command became; letting the call through would run \
         a second shell that dies with the turn: {decision}"
    );
    assert!(
        !queued.contains("bg-1"),
        "the person was asked whether to monitor a command Keel had already taken off the turn: \
         {queued}"
    );
    assert!(
        jobs.contains("\"exit\":0") && jobs.contains("done"),
        "the job did not finish under Keel with its output kept — which is the entire point: \
         {jobs}"
    );
    assert!(
        !other.contains("bg-1") && other.trim() == "[]",
        "another conversation was shown this one's job: {other}"
    );
}

/// 22. A lane can read the project it is a lane of.
///
/// A lane runs in `<repo>/.keel/worktrees/<name>`, so everything at the project root is outside
/// the agent's working directory — including `.keel/attachments`, where Keel writes the file the
/// person has just dragged into the chat. Found in a real transcript: the agent asked to read the
/// attachment it had been handed and was told it had no permission, with no card to click,
/// because `Read` is not a tool the hook covers.
#[test]
fn eval_22_a_lane_can_still_read_the_project_it_belongs_to() {
    let (repo, port, mut daemon, argv) = stubbed_project();
    post(port, "/api/worktree/create", r#"{"name":"eval-add-dir"}"#);
    let _ = curl_stream(&format!(
        "http://127.0.0.1:{port}/api/chat?provider=claude&mode=plan&lane=l&wt=eval-add-dir&prompt={}",
        urlencode("hello")
    ));
    let line = argv_of(&argv);
    let root = repo.path().canonicalize().expect("canonical repo");
    stop_daemon(&mut daemon);

    assert!(
        line.contains("--add-dir"),
        "a lane was given no way to read the project root, so an attachment it was handed is \
         unreadable:\n{line}"
    );
    assert!(
        line.contains(root.to_str().expect("utf8")),
        "`--add-dir` was passed something other than the project:\n{line}"
    );
}

/// 23. A conversation belongs to the project it was started in.
///
/// History lists sessions from a shared parent directory, so two repositories under `~/Dev` are
/// each other's neighbours and one project's session is one click away in the other. Resuming it
/// used to fall back silently to the open project: verified in a real transcript, where one
/// session id carries `cwd` changing from one project's worktree to another's mid-file and every
/// tool call after the switch comes back "you haven't granted permissions to read from …".
#[test]
fn eval_23_a_session_from_another_project_is_refused_not_relocated() {
    let (_repo, port, mut daemon) = project(&[("Makefile", "check:\n\ttrue\n")]);
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let stream = curl_stream(&format!(
        "http://127.0.0.1:{port}/api/chat?provider=claude&mode=plan&lane=l&session=abc-123&cwd={}&prompt={}",
        urlencode(elsewhere.path().to_str().expect("utf8")),
        urlencode("carry on")
    ));
    stop_daemon(&mut daemon);

    assert!(
        stream.contains("fatal"),
        "another project's conversation was resumed here rather than refused — every path it \
         already knows is unreadable, and what the person sees is the agent saying it has no \
         access to a folder:\n{stream}"
    );
    assert!(
        stream.contains("not the project open here"),
        "the refusal did not say why, which is the half that makes it actionable:\n{stream}"
    );
}

/// A stubbed project with extra files in place before the daemon starts.
fn stubbed_project_with(
    files: &[(&str, &str)],
) -> (tempfile::TempDir, u16, Child, std::path::PathBuf) {
    let (repo, port, daemon, argv) = stubbed_project();
    let dir = repo.path();
    for (name, body) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }
    (repo, port, daemon, argv)
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
