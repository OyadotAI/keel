//! Checking whether the repository actually still works.
//!
//! An agent reporting "done" is an assertion, not evidence. Research on agentic coding is blunt
//! about this: self-verification is close to worthless, and the cheapest gate that genuinely works
//! is running the project's own checks and reading the exit code.
//!
//! So Keel runs them itself after every turn. The agent does not get to grade its own work.

use axum::response::sse::{Event, Sse};
use camino::Utf8Path;
use serde::Serialize;
use std::convert::Infallible;
use std::process::Stdio;
use tokio::process::Command;
use tokio_stream::wrappers::ReceiverStream;

use crate::serve::Checkout;

/// How this repository proves itself.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// Shown to the user, and run through the shell.
    pub command: String,
    /// Why Keel picked it.
    pub source: &'static str,
}

/// Work out how to verify a repository, in order of how much the project itself has decided.
///
/// A `check` target in a Makefile is the project stating its own gate, so it wins. Otherwise the
/// package scripts, then the language default. Guessing beyond that would run something the author
/// never meant as a gate.
pub fn detect(root: &Utf8Path) -> Option<Check> {
    if let Ok(makefile) = std::fs::read_to_string(root.join("Makefile"))
        && makefile.lines().any(|l| l.starts_with("check:"))
    {
        return Some(Check {
            command: "make check".into(),
            source: "the `check` target in your Makefile",
        });
    }

    // A justfile is the same statement as a Makefile, made by people who did not want make.
    if let Ok(justfile) = std::fs::read_to_string(root.join("justfile"))
        .or_else(|_| std::fs::read_to_string(root.join("Justfile")))
        && justfile.lines().any(|l| l.starts_with("check:"))
    {
        return Some(Check {
            command: "just check".into(),
            source: "the `check` recipe in your justfile",
        });
    }

    if let Ok(text) = std::fs::read_to_string(root.join("package.json"))
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
    {
        let scripts = json.get("scripts").cloned().unwrap_or_default();
        let has = |k: &str| scripts.get(k).is_some();
        // The lockfile names the package manager. Running `npm run` in a pnpm workspace works
        // until a script uses a workspace protocol and then does not, which is a confusing way to
        // find out that Keel guessed.
        let runner = if root.join("bun.lock").exists() || root.join("bun.lockb").exists() {
            "bun run"
        } else if root.join("pnpm-lock.yaml").exists() {
            "pnpm run"
        } else if root.join("yarn.lock").exists() {
            "yarn run"
        } else {
            "npm run"
        };

        // A `check` script is the project naming its own gate, the same as a Makefile target.
        if has("check") {
            return Some(Check {
                command: format!("{runner} check"),
                source: "the `check` script in your package.json",
            });
        }

        let mut parts = Vec::new();
        if has("typecheck") {
            parts.push(format!("{runner} typecheck"));
        }
        if has("lint") {
            parts.push(format!("{runner} lint"));
        }
        if has("test") {
            // `bun run test` and `bun test` differ; the script is what the author wrote.
            parts.push(format!("{runner} test"));
        }
        if !parts.is_empty() {
            return Some(Check {
                command: parts.join(" && "),
                source: "the scripts in your package.json",
            });
        }
    }

    if root.join("Cargo.toml").exists() {
        return Some(Check {
            command: "cargo test".into(),
            source: "this being a Cargo project",
        });
    }

    // Python only when there is something to run: `pytest` with no tests exits 5, and a gate that
    // fails because the project has no tests reports the wrong thing every turn.
    let has_tests = root.join("tests").is_dir() || root.join("test").is_dir();
    if has_tests
        && (root.join("pyproject.toml").exists()
            || root.join("pytest.ini").exists()
            || root.join("setup.cfg").exists())
    {
        let command = if root.join("uv.lock").exists() {
            "uv run pytest -q"
        } else if root.join("poetry.lock").exists() {
            "poetry run pytest -q"
        } else {
            "pytest -q"
        };
        return Some(Check {
            command: command.into(),
            source: "the tests in this Python project",
        });
    }

    if root.join("go.mod").exists() {
        return Some(Check {
            command: "go test ./...".into(),
            source: "this being a Go module",
        });
    }

    None
}

/// One diagnostic, located in a file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Problem {
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// `error` or `warning`.
    pub severity: &'static str,
    pub message: String,
}

/// Pull a diagnostic out of one line of tool output.
///
/// Three shapes cover the toolchains this IDE targets, and they are matched by structure rather
/// than by asking each tool for JSON — `make check` chains several tools together and the output
/// arrives interleaved, so the parser has to work on whatever comes past.
pub fn parse_problem(line: &str) -> Option<Problem> {
    let trimmed = line.trim();

    // tsc:  src/index.ts(12,5): error TS2307: Cannot find module 'x'.
    if let Some((path, rest)) = trimmed.split_once('(')
        && let Some((pos, tail)) = rest.split_once("): ")
        && let Some((l, c)) = pos.split_once(',')
        && let (Ok(line_no), Ok(col)) = (l.trim().parse(), c.trim().parse())
    {
        let severity = if tail.starts_with("warning") {
            "warning"
        } else {
            "error"
        };
        return Some(Problem {
            file: path.trim().to_string(),
            line: line_no,
            col,
            severity,
            message: tail.trim().to_string(),
        });
    }

    // rustc/cargo:  --> crates/keel/src/main.rs:42:9
    if let Some(loc) = trimmed.strip_prefix("--> ") {
        let mut parts = loc.rsplitn(3, ':');
        if let (Some(c), Some(l), Some(path)) = (parts.next(), parts.next(), parts.next())
            && let (Ok(line_no), Ok(col)) = (l.parse(), c.parse())
        {
            return Some(Problem {
                file: path.to_string(),
                line: line_no,
                col,
                severity: "error",
                message: String::new(), // filled from the preceding error line
            });
        }
    }

    // eslint/gcc style:  src/x.ts:12:5: error: message
    let mut parts = trimmed.splitn(4, ':');
    if let (Some(path), Some(l), Some(c), Some(rest)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
        && let (Ok(line_no), Ok(col)) = (l.trim().parse::<u32>(), c.trim().parse::<u32>())
        && path.contains('/')
    {
        let rest = rest.trim();
        let severity = if rest.starts_with("warning") {
            "warning"
        } else {
            "error"
        };
        return Some(Problem {
            file: path.trim().to_string(),
            line: line_no,
            col,
            severity,
            message: rest
                .trim_start_matches("error:")
                .trim_start_matches("warning:")
                .trim()
                .to_string(),
        });
    }

    None
}

/// Report which check would run, without running it.
pub async fn plan(Checkout(repo): Checkout) -> axum::Json<Option<Check>> {
    axum::Json(detect(&repo))
}

/// Run the check and stream its output.
pub async fn run(Checkout(repo): Checkout) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(256);

    tokio::spawn(async move {
        let Some(check) = detect(&repo) else {
            let _ = tx
                .send(Ok(Event::default().event("none").data(
                    "No check command found. Add a `check` target to your Makefile, or \
                     typecheck/test scripts to package.json.",
                )))
                .await;
            return;
        };

        let _ = tx
            .send(Ok(Event::default()
                .event("start")
                .data(check.command.clone())))
            .await;

        // Through a shell, because the detected command is a pipeline of the project's own scripts.
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&check.command)
            .current_dir(&repo)
            // Nothing to read from. The gate inherited the daemon's stdin, so a check that asks a
            // question — a prompt, a confirmation, a login — blocked forever with the gate stuck
            // on "running" and no way to end it. There is nobody to answer it here.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send(Ok(Event::default().event("fatal").data(e.to_string())))
                    .await;
                return;
            }
        };

        let (out, err) = (child.stdout.take(), child.stderr.take());
        tokio::join!(scan_pipe(out, tx.clone()), scan_pipe(err, tx.clone()));

        let code = child
            .wait()
            .await
            .map(|s| s.code().unwrap_or(-1))
            .unwrap_or(-1);
        let _ = tx
            .send(Ok(Event::default().event("done").data(code.to_string())))
            .await;
    });

    Sse::new(ReceiverStream::new(rx))
}

/// Forward a pipe line by line, emitting a `problem` event whenever a line locates one.
///
/// Cargo prints the message on one line and the location on the next, so the last message seen is
/// carried forward to fill an otherwise-empty location.
async fn scan_pipe<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    tx: tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let Some(pipe) = pipe else { return };
    let mut lines = BufReader::new(pipe).lines();
    let mut last_message = String::new();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.starts_with("error") || trimmed.starts_with("warning") {
            last_message = trimmed.to_string();
        }

        if let Some(mut p) = parse_problem(&line) {
            if p.message.is_empty() {
                p.message = std::mem::take(&mut last_message);
            }
            if let Ok(json) = serde_json::to_string(&p)
                && tx
                    .send(Ok(Event::default().event("problem").data(json)))
                    .await
                    .is_err()
            {
                return;
            }
        }

        if tx
            .send(Ok(Event::default().event("line").data(line)))
            .await
            .is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use tempfile::TempDir;

    fn repo(files: &[(&str, &str)]) -> (TempDir, Utf8PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        for (p, body) in files {
            std::fs::write(root.join(p), body).expect("write");
        }
        (dir, root)
    }

    /// Every gate a project can already be stating, so Keel stops reporting "no check command" at
    /// a repository that has one under a name it did not know.
    #[test]
    fn a_project_that_declares_a_gate_is_believed() {
        let (_d, just) = repo(&[("justfile", "check:\n\tcargo test\n")]);
        assert_eq!(detect(&just).unwrap().command, "just check");

        let (_d, script) = repo(&[("package.json", r#"{"scripts":{"check":"tsc && vitest"}}"#)]);
        assert_eq!(detect(&script).unwrap().command, "npm run check");

        let (_d, pnpm) = repo(&[
            ("package.json", r#"{"scripts":{"check":"turbo check"}}"#),
            ("pnpm-lock.yaml", ""),
        ]);
        assert_eq!(detect(&pnpm).unwrap().command, "pnpm run check");

        let (_d, yarn) = repo(&[
            ("package.json", r#"{"scripts":{"test":"jest"}}"#),
            ("yarn.lock", ""),
        ]);
        assert_eq!(detect(&yarn).unwrap().command, "yarn run test");

        let (_d, go) = repo(&[("go.mod", "module x\n")]);
        assert_eq!(detect(&go).unwrap().command, "go test ./...");
    }

    /// Python needs tests to exist before running pytest is a gate rather than an error: with no
    /// tests collected it exits 5, which would fail every turn for a reason nobody caused.
    #[test]
    fn python_is_a_gate_only_when_there_is_something_to_run() {
        let (_d, bare) = repo(&[("pyproject.toml", "[project]\nname='x'\n")]);
        assert!(detect(&bare).is_none());

        let (_d2, root) = repo(&[("pyproject.toml", "[project]\nname='x'\n")]);
        std::fs::create_dir(root.join("tests")).unwrap();
        assert_eq!(detect(&root).unwrap().command, "pytest -q");

        std::fs::write(root.join("uv.lock"), "").unwrap();
        assert_eq!(detect(&root).unwrap().command, "uv run pytest -q");
    }

    #[test]
    fn reads_a_typescript_diagnostic() {
        let p =
            parse_problem("src/index.test.ts(1,30): error TS2307: Cannot find module 'bun:test'.")
                .expect("parsed");
        assert_eq!(p.file, "src/index.test.ts");
        assert_eq!((p.line, p.col), (1, 30));
        assert_eq!(p.severity, "error");
        assert!(p.message.contains("TS2307"));
    }

    #[test]
    fn reads_a_rust_location() {
        let p = parse_problem("  --> crates/keel/src/main.rs:42:9").expect("parsed");
        assert_eq!(p.file, "crates/keel/src/main.rs");
        assert_eq!((p.line, p.col), (42, 9));
        // The message arrives on the preceding line and is filled in by the scanner.
        assert!(p.message.is_empty());
    }

    #[test]
    fn reads_a_colon_style_diagnostic() {
        let p = parse_problem("src/app.ts:10:4: warning: unused variable").expect("parsed");
        assert_eq!(p.file, "src/app.ts");
        assert_eq!(p.severity, "warning");
        assert_eq!(p.message, "unused variable");
    }

    #[test]
    fn ordinary_output_is_not_mistaken_for_a_diagnostic() {
        assert!(parse_problem("Ran 3 tests across 1 file. [64.00ms]").is_none());
        assert!(parse_problem("$ tsc --noEmit").is_none());
        assert!(parse_problem(" 3 pass").is_none());
        assert!(parse_problem("").is_none());
    }

    #[test]
    fn a_makefile_check_target_wins() {
        // The project stating its own gate beats anything Keel would infer.
        let (_d, root) = repo(&[
            ("Makefile", "check: fmt lint test\n"),
            ("Cargo.toml", "[package]\nname=\"x\"\n"),
        ]);
        assert_eq!(detect(&root).unwrap().command, "make check");
    }

    #[test]
    fn package_scripts_are_chained_in_order() {
        let (_d, root) = repo(&[(
            "package.json",
            r#"{"scripts":{"test":"bun test","typecheck":"tsc --noEmit"}}"#,
        )]);
        assert_eq!(
            detect(&root).unwrap().command,
            "npm run typecheck && npm run test"
        );
    }

    #[test]
    fn a_bun_lockfile_selects_bun() {
        let (_d, root) = repo(&[
            ("package.json", r#"{"scripts":{"test":"bun test"}}"#),
            ("bun.lock", ""),
        ]);
        assert_eq!(detect(&root).unwrap().command, "bun run test");
    }

    #[test]
    fn a_cargo_project_falls_back_to_cargo_test() {
        let (_d, root) = repo(&[("Cargo.toml", "[package]\nname=\"x\"\n")]);
        assert_eq!(detect(&root).unwrap().command, "cargo test");
    }

    #[test]
    fn a_repository_with_no_gate_says_so_rather_than_guessing() {
        let (_d, root) = repo(&[("README.md", "hi")]);
        assert!(detect(&root).is_none());
    }
}
