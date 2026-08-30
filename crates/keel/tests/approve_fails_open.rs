//! The approval hook fails open, tested against the real binary.
//!
//! `CLAUDE.md` says of this hook: "It fails open. Keel unreachable, socket dropped, nobody at the
//! keyboard — every path prints nothing and exits 0, which defers to the allowlist. A guardrail
//! that can wedge the agent is one people turn off."
//!
//! Every path *inside* `main` did that. The one outside it did not: `--lane` was added to the
//! hook's command line before the binary accepted it, so clap exited 2 on `error: unexpected
//! argument '--lane' found` before any of that code ran — and every `Bash` call in every session
//! was refused. An old hook on disk invoking a newer binary is the same shape, and will happen
//! again; this is what makes the sentence above true rather than intended.
//!
//! Run against the built binary rather than a function, because the failure was in argument
//! parsing, which a unit test does not reach.

use std::io::Write;
use std::process::{Command, Stdio};

/// A port nothing is listening on: the hook must defer, not hang and not fail.
const DEAD_PORT: &str = "59997";

fn approve(args: &[&str], stdin: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_keel"))
        .arg("approve")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("could not run the keel binary");
    child
        .stdin
        .take()
        .expect("no stdin")
        .write_all(stdin.as_bytes())
        .expect("could not write the hook input");
    child.wait_with_output().expect("could not wait for keel")
}

const BASH: &str = r#"{"tool_name":"Bash","tool_input":{"command":"ls"},"session_id":"S"}"#;

#[test]
fn an_argument_this_binary_does_not_know_still_defers() {
    let out = approve(
        &["--port", DEAD_PORT, "--invented-in-a-later-version", "x"],
        BASH,
    );
    assert!(
        out.status.success(),
        "an unparseable approval exited {:?} — every Bash call in every session is now refused",
        out.status.code()
    );
    assert!(
        out.stdout.is_empty(),
        "a deferring hook must print nothing, got {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// The specific flag that broke it, which the hook has been passing since it was added.
#[test]
fn the_lane_the_hook_passes_is_accepted() {
    let out = approve(&["--port", DEAD_PORT, "--lane", "LANE-A"], BASH);
    assert!(
        out.status.success(),
        "`--lane` was rejected: {:?}",
        out.status.code()
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("unexpected argument"),
        "clap rejected the hook's own command line: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A lane with no id yet is written as `--lane ` by the hook, and that is not an error either.
#[test]
fn an_empty_lane_is_not_an_error() {
    let out = approve(&["--port", DEAD_PORT, "--lane", ""], BASH);
    assert!(out.status.success());
}

/// Unreachable Keel is the case the doc comment names first.
#[test]
fn an_unreachable_keel_defers() {
    let out = approve(&["--port", DEAD_PORT, "--lane", "L"], BASH);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

/// Nonsense on stdin is not a reason to block a command either.
#[test]
fn input_that_is_not_a_hook_payload_defers() {
    let out = approve(&["--port", DEAD_PORT, "--lane", "L"], "not json at all");
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

/// The fail-open is scoped to the hook. A typo on an ordinary command still says so, or every
/// mistyped `keel scan` would exit 0 and look like it worked.
#[test]
fn an_ordinary_command_still_reports_a_bad_argument() {
    let out = Command::new(env!("CARGO_BIN_EXE_keel"))
        .args(["scan", "--not-a-real-flag"])
        .output()
        .expect("could not run the keel binary");
    assert!(!out.status.success(), "a mistyped command exited 0");
    assert!(String::from_utf8_lossy(&out.stderr).contains("unexpected argument"));
}
