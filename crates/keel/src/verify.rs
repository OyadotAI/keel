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
    /// Where to run it, relative to the opened folder. Empty is the folder itself.
    ///
    /// Every probe here used to be `root.join(...)`, so a workspace holding `backend/` and
    /// `frontend/` — each with its own tests — detected nothing, the gate never ran, and
    /// auto-commit committed the agent's work ungated. `dev.rs` learned this lesson; this had not.
    #[serde(default)]
    pub dir: String,
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
            dir: String::new(),
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
            dir: String::new(),
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
                dir: String::new(),
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
                dir: String::new(),
            });
        }
    }

    if root.join("Cargo.toml").exists() {
        return Some(Check {
            command: "cargo test".into(),
            source: "this being a Cargo project",
            dir: String::new(),
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
            dir: String::new(),
        });
    }

    if root.join("go.mod").exists() {
        return Some(Check {
            command: "go test ./...".into(),
            source: "this being a Go module",
            dir: String::new(),
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
        && looks_like_a_file(path)
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

/// Whether this is a path rather than the left-hand side of something that merely has colons in
/// it, like a timestamp.
///
/// It used to require a `/`, which is wrong for a file at the top of its own project — `app.ts:3:1`
/// from a tool run inside `frontend/`. That was survivable while every gate ran at the repository
/// root and most paths had a directory in them; in a folder of projects it silently dropped the
/// problems of whichever half keeps its sources at the top.
fn looks_like_a_file(path: &str) -> bool {
    if path.contains('/') {
        return true;
    }
    // An extension, and a name in front of it: `app.ts` yes, `12` no, `1.5` no.
    match path.trim().rsplit_once('.') {
        Some((name, ext)) => {
            !name.is_empty()
                && !name.chars().all(|c| c.is_ascii_digit())
                && (1..=5).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic())
        }
        None => false,
    }
}

/// Report which check would run, without running it.
pub async fn plan(Checkout(repo): Checkout) -> axum::Json<Option<Check>> {
    axum::Json(detect(&repo))
}

/// Every check this folder has: the project's own, or one per repository it holds.
///
/// The root wins when it declares one — a workspace with a Makefile that runs both halves is the
/// project stating its own gate, exactly as `detect` treats it. Only when the root says nothing
/// does each repository answer for itself, and then all of them run.
pub fn detect_all(root: &Utf8Path) -> Vec<Check> {
    if let Some(check) = detect(root) {
        return vec![check];
    }
    let mut out = Vec::new();
    for found in crate::gitroots::find(root) {
        if found.dir.is_empty() {
            continue;
        }
        if let Some(mut check) = detect(&root.join(&found.dir)) {
            check.dir = found.dir.clone();
            out.push(check);
        }
    }
    out
}

/// Run the check and stream its output.
pub async fn run(Checkout(repo): Checkout) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(256);

    tokio::spawn(async move {
        let checks = detect_all(&repo);
        if checks.is_empty() {
            let _ = tx
                .send(Ok(Event::default().event("none").data(
                    "No check command found. Add a `check` target to your Makefile, or \
                     typecheck/test scripts to package.json.",
                )))
                .await;
            return;
        }

        // Every repository's gate, in turn. One `done` at the end carrying the worst code, so a
        // green frontend cannot hide a red backend — and so the app needs no change to read it.
        let mut worst = 0;
        for check in &checks {
            let where_ = if check.dir.is_empty() {
                check.command.clone()
            } else {
                format!("{} · {}", check.dir, check.command)
            };
            let _ = tx
                .send(Ok(Event::default().event("start").data(where_)))
                .await;

            // Through a shell, because the detected command is a pipeline of the project's own
            // scripts.
            let mut cmd = Command::new("sh");
            cmd.arg("-c")
                .arg(&check.command)
                .current_dir(repo.join(&check.dir))
                // Nothing to read from. The gate inherited the daemon's stdin, so a check that
                // asks a question — a prompt, a confirmation, a login — blocked forever with the
                // gate stuck on "running" and no way to end it. There is nobody to answer it here.
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
            tokio::join!(
                scan_pipe(out, tx.clone(), &check.dir),
                scan_pipe(err, tx.clone(), &check.dir)
            );

            let code = child
                .wait()
                .await
                .map(|s| s.code().unwrap_or(-1))
                .unwrap_or(-1);
            if code != 0 && worst == 0 {
                worst = code;
            }
        }

        let _ = tx
            .send(Ok(Event::default().event("done").data(worst.to_string())))
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
    dir: &str,
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
            // A problem in a workspace has to say which repository it is in, or clicking it opens
            // a path that does not exist from where the person is looking.
            //
            // Defensively, because tools disagree about what they print: some paths are relative
            // to the directory the command ran in, some are already relative to the repository,
            // and some are absolute. Prefixing blindly produced `frontend/frontend/app.ts`.
            if !dir.is_empty()
                && !p.file.starts_with('/')
                && !p.file.starts_with(&format!("{dir}/"))
            {
                p.file = format!("{dir}/{}", p.file);
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
mod workspace_tests {
    use super::*;
    use camino::Utf8PathBuf;

    fn tmp(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .unwrap()
            .join(format!("keel-gate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn repo_with(at: &Utf8Path, file: &str, body: &str) {
        std::fs::create_dir_all(at).unwrap();
        std::fs::write(at.join(file), body).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@e.com"],
            vec!["config", "user.name", "t"],
        ] {
            std::process::Command::new("git")
                .current_dir(at)
                .args(args)
                .output()
                .unwrap();
        }
    }

    /// The hole this closes: every probe was `root.join(...)`, so a folder holding two projects
    /// detected nothing, the gate never ran, and auto-commit committed the agent's work ungated.
    #[test]
    fn each_repository_in_a_workspace_gets_its_own_gate() {
        let root = tmp("workspace");
        repo_with(&root.join("backend"), "Makefile", "check:\n\techo ok\n");
        repo_with(
            &root.join("frontend"),
            "package.json",
            r#"{"scripts":{"test":"echo ok"}}"#,
        );

        let checks = detect_all(&root);
        assert_eq!(checks.len(), 2, "one gate per repository, not none");
        let dirs: Vec<_> = checks.iter().map(|c| c.dir.as_str()).collect();
        assert_eq!(dirs, ["backend", "frontend"]);
        assert_eq!(checks[0].command, "make check");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A workspace whose root declares its own gate is the project stating it, and that wins —
    /// the same rule `detect` has always followed, and the same one `dev::detect` follows.
    #[test]
    fn a_gate_at_the_root_still_wins() {
        let root = tmp("rootwins");
        std::fs::write(root.join("Makefile"), "check:\n\techo both\n").unwrap();
        repo_with(&root.join("backend"), "Makefile", "check:\n\techo be\n");

        let checks = detect_all(&root);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].command, "make check");
        assert_eq!(
            checks[0].dir, "",
            "run at the root, which is what it describes"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A tool run inside a sub-project prints a path with no directory in it, and requiring a `/`
    /// dropped every one of those problems on the floor.
    #[test]
    fn a_file_at_the_top_of_its_project_is_still_a_problem() {
        let p = parse_problem("app.ts:3:1: error: something is wrong").expect("a problem");
        assert_eq!(p.file, "app.ts");
        assert_eq!((p.line, p.col), (3, 1));
    }

    /// And the reason the `/` was there: things with colons that are not files.
    #[test]
    fn a_timestamp_is_not_a_problem() {
        assert!(parse_problem("12:34:56 building…").is_none());
        assert!(parse_problem("make: *** [check] Error 1").is_none());
        assert!(parse_problem("warning: 3 targets:1:1 skipped").is_none());
    }

    /// And an ordinary project is untouched.
    #[test]
    fn one_project_detects_exactly_one_gate() {
        let root = tmp("single");
        std::fs::write(root.join("Makefile"), "check:\n\techo ok\n").unwrap();
        let checks = detect_all(&root);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].dir, "");
        let _ = std::fs::remove_dir_all(&root);
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
