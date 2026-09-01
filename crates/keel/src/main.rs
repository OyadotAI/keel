//! Keel — the local IDE that makes a repo shippable.
//!
//! At Milestone 0 only `keel scan` is wired end to end. That is deliberate: the readiness scan is
//! useful with nothing connected, so it ships and is trusted before Keel is ever handed a cloud
//! credential.

mod agent;
mod agents;
mod approve;
mod askmcp;
mod aws;
mod clitools;
mod connect;
mod dev;
mod fsops;
mod git;
mod gitroots;
mod ignored;
mod imports;
mod lines;
mod lock;
mod mcp;
mod monitor;
mod names;
mod pair;
mod path;
mod permissions;
mod plugins;
mod policy;
mod pr;
mod prefs;
mod project;
mod render;
mod repo;
mod review;
mod serve;
mod signals;
mod snapshot;
mod term;
mod tree;
mod turns;
mod verify;
mod worktree;

use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use keel_harness::quarantine;
use keel_scanner::{RepoContext, scan};
use keel_workspace::Workspace;

#[derive(Parser)]
#[command(
    name = "keel",
    version,
    about = "The daemon behind Keel, the agentic development environment"
)]
struct Cli {
    /// Absent when launched from the Dock, where a bundle's executable is run with no arguments.
    /// That is the application, so that is what a bare `keel` does.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Audit a repository for agent- and production-readiness.
    Scan {
        /// Repository to scan. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: Utf8PathBuf,

        /// Emit JSON instead of a rendered report.
        #[arg(long)]
        json: bool,

        /// Exit non-zero when anything critical or high is outstanding, for CI.
        #[arg(long)]
        strict: bool,
    },

    /// Show everything Claude Code knows about this repository.
    ///
    /// Sessions, skills, plugins, subagents and commands all shape how an agent behaves here, and
    /// none of them are visible while you work. This is the inventory.
    Workspace {
        /// Repository to inspect. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: Utf8PathBuf,

        /// Emit JSON instead of a rendered listing.
        #[arg(long)]
        json: bool,
    },

    /// List Claude Code sessions recorded for this repository.
    Sessions {
        #[arg(default_value = ".")]
        path: Utf8PathBuf,

        /// Show every session rather than the ten most recent.
        #[arg(long)]
        all: bool,
    },

    /// Run the daemon the Keel app talks to, on loopback.
    ///
    /// An HTTP surface, not a UI: the window is the Mac app, and this opens nothing.
    Serve {
        /// Repository to open. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: Utf8PathBuf,

        #[arg(long, default_value_t = 7777)]
        port: u16,

        /// Accepted and ignored. `serve` never opens anything now — there is no page to open.
        ///
        /// Kept because clap exits 2 on a flag it does not know, and a `PreToolUse` hook reads a
        /// non-zero exit as *block*; a daemon that refuses to start over a stale flag in somebody's
        /// script is the silent failure this flag's own removal was meant to end.
        #[arg(long, hide = true)]
        no_open: bool,

        /// Exit when the process that started this one goes away.
        ///
        /// For the Mac app, which spawns the daemon as a child. macOS has no `PR_SET_PDEATHSIG`,
        /// and a terminating app does not always get to run cleanup — SIGTERM, a force quit and a
        /// crash all skip it. Without this the daemon reparents to init and keeps serving, which
        /// is how a machine accumulates one invisible agent host per app launch.
        #[arg(long)]
        exit_with_parent: bool,

        /// Report panics to Sentry. Passed by the app from its own configuration, so the
        /// daemon reports under the same key with `component=daemon`.
        #[arg(long)]
        sentry_dsn: Option<String>,

        /// Reopen the last project instead of reading `path`.
        ///
        /// For a GUI launch, which has no working directory worth inferring a project from — from
        /// Finder it is `/`, so the default `.` would open the whole filesystem as a repository.
        /// `keel serve` in a terminal keeps meaning "this directory", because there it does.
        #[arg(long)]
        resume_last: bool,
    },

    /// Answer a Claude Code `PreToolUse` hook by asking the running Keel.
    ///
    /// Not typed by anyone: Keel writes this into the settings it passes to `claude`, and Claude
    /// Code runs it before every matching tool call and blocks on the result. It reads the hook's
    /// JSON on stdin and prints a permission decision.
    #[command(hide = true)]
    Approve {
        /// The port the Keel that spawned this agent is serving on.
        #[arg(long)]
        port: u16,
        /// The window that started this turn.
        ///
        /// Claude Code does not send it, so Keel puts it on this command line — and it is the only
        /// id that exists on a lane's *first* turn, when the Claude session id the queue would
        /// otherwise partition by has not been assigned yet. Optional, because the flag was added
        /// to the hook before it was accepted here and an older hook may still be on disk.
        #[arg(long, default_value = "")]
        lane: String,
        /// The checkout the turn is running in.
        ///
        /// Same reason as `lane`: Claude Code does not send it, and Keel needs it to run a
        /// monitored command where the agent would have run it — a lane's worktree, not the
        /// project root. Empty falls back to the project.
        #[arg(long, default_value = "")]
        cwd: String,
    },

    /// Move repository-supplied agent configuration out of the way.
    ///
    /// Run before pointing any agent at a repository you did not write. Claude Code loads a repo's
    /// own `.claude/settings.json` hooks and executes them with no trust prompt, so this is what
    /// stands between a hostile repo and code execution on this machine.
    Trust {
        /// Repository to quarantine. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: Utf8PathBuf,
    },
}

/// Resolve the repository path and read Claude Code's state for it.
fn discover(path: &Utf8PathBuf) -> Result<Workspace> {
    let home = keel_workspace::claude_home()
        .context("could not locate the Claude Code home directory (is HOME set?)")?;

    // Session transcripts are filed under the absolute working directory, so a relative path finds
    // nothing until it is canonicalised.
    let repo = path
        .canonicalize_utf8()
        .with_context(|| format!("resolving {path}"))?;

    Ok(Workspace::discover(&repo, &home))
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("KEEL_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    // Before anything is spawned. A Dock launch inherits launchd's PATH, which has none of the
    // places these tools install to, and every `Command::new` after this point depends on it.
    path::adopt_shell_path();

    // macOS hands a bundled process a `-psn_0_…` serial number on some launches. It is not an
    // argument anyone typed, and clap would reject it and take the application down on start.
    let args: Vec<_> = std::env::args_os()
        .filter(|a| !a.to_string_lossy().starts_with("-psn_"))
        .collect();

    // The approval hook fails open, and that has to include failing to parse its own arguments.
    //
    // `--lane` was added to the hook's command line before it was accepted here, so every `Bash`
    // call died on `error: unexpected argument '--lane' found` — clap exits(2) long before the
    // code that knows to defer, so the guardrail that documents itself as never able to wedge the
    // agent wedged it completely. An old hook on disk invoking a new binary is the same shape.
    //
    // Anything unparseable that was trying to be an approval exits 0, which defers to Claude
    // Code's own permission check.
    //
    // It used to print nothing as well, and that half was wrong. Deferring means the allowlist
    // decides alone: every command outside it is refused, no card is ever queued, and nobody
    // learns that the hook is dead — it reads as "Keel keeps rejecting me" with no question to
    // answer. `--cwd '<path>'` unquoted did exactly that to any project whose path had a space in
    // it, and the report that reached us was a tester saying the agent had no access to a folder.
    // One line on stderr is the difference between an hour and never.
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(e) => {
            if args.iter().any(|a| a == "approve") {
                eprintln!(
                    "keel: the approval hook could not read its own arguments, so this call is \
                     being left to the allowlist. Anything outside it will be refused without \
                     asking. Update Keel, or reopen the project."
                );
                return Ok(());
            }
            e.exit();
        }
    };

    // No subcommand used to mean "open the window", which is not this binary's job any more —
    // the window is the Mac app, and this is the daemon it drives. Help beats guessing.
    let Some(command) = cli.command else {
        use clap::CommandFactory;
        Cli::command().print_help()?;
        return Ok(());
    };

    match command {
        Command::Scan { path, json, strict } => {
            let ctx = RepoContext::load(&path)
                .with_context(|| format!("reading repository at {path}"))?;
            let report = scan(&ctx);

            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", render::report(&report));
            }

            if strict && !report.is_shippable() {
                std::process::exit(1);
            }
        }

        Command::Serve {
            path,
            port,
            no_open: _,
            exit_with_parent,
            sentry_dsn,
            resume_last,
        } => {
            // Held for the life of the process: dropping the guard flushes and stops reporting.
            let _sentry = sentry_dsn.filter(|d| !d.is_empty()).map(|dsn| {
                let guard = sentry::init((
                    dsn,
                    sentry::ClientOptions {
                        release: Some(format!("keel@{}", env!("CARGO_PKG_VERSION")).into()),
                        ..Default::default()
                    },
                ));
                sentry::configure_scope(|s| {
                    s.set_tag("app", "keel");
                    s.set_tag("component", "daemon");
                    s.set_tag("version", env!("CARGO_PKG_VERSION"));
                });
                guard
            });
            if exit_with_parent {
                watch_parent();
            }
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("starting the async runtime")?;
            if resume_last {
                runtime.block_on(serve::run_app(port))?;
            } else {
                let repo = path
                    .canonicalize_utf8()
                    .with_context(|| format!("resolving {path}"))?;
                runtime.block_on(serve::run(repo, port))?;
            }
        }

        Command::Approve { port, lane, cwd } => {
            // Every failure here prints nothing and exits 0, which defers to Claude Code's own
            // permission check. A guardrail that can wedge the agent is one people disable.
            let mut raw = String::new();
            if std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw).is_err() {
                return Ok(());
            }
            let Ok(mut hook) = serde_json::from_str::<approve::HookInput>(&raw) else {
                return Ok(());
            };
            // Not in what Claude Code sends; they arrive on the command line instead.
            hook.lane = lane;
            hook.cwd = cwd;

            let decision = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("starting the async runtime")?
                .block_on(approve::request(port, &hook));

            if let Some(d) = decision
                && d.decision != "defer"
            {
                println!(
                    "{}",
                    serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": d.decision,
                            "permissionDecisionReason": d.reason,
                        }
                    })
                );
            }
        }

        Command::Workspace { path, json } => {
            let workspace = discover(&path)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&workspace)?);
            } else {
                print!("{}", render::workspace(&workspace));
            }
        }

        Command::Sessions { path, all } => {
            let workspace = discover(&path)?;
            print!("{}", render::sessions(&workspace, all));
        }

        Command::Trust { path } => {
            let report = quarantine(&path)
                .with_context(|| format!("quarantining agent config in {path}"))?;

            if report.is_clean() {
                println!("\nNothing to quarantine — this repository ships no agent config.\n");
                return Ok(());
            }

            println!("\nQuarantined {} item(s):\n", report.quarantined.len());
            for item in &report.quarantined {
                println!("  {item}");
            }
            if let Some(location) = &report.location {
                println!("\nMoved to {location}");
            }
            println!(
                "\nRead each one before restoring it. Hooks run shell commands at session start,\n\
                 and MCP servers are connected automatically.\n"
            );
        }
    }

    Ok(())
}

/// Exit once the process that started this one is gone.
///
/// A polled `getppid()` rather than anything cleverer: on Unix an orphan is reparented to pid 1,
/// so the parent going away is a one-integer comparison. A second of latency is irrelevant for a
/// process whose job is now to stop existing, and this catches the cases a cleanup handler cannot
/// — SIGKILL, a force quit, and a crashed parent.
fn watch_parent() {
    let original = std::os::unix::process::parent_id();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let now = std::os::unix::process::parent_id();
            if now != original || now == 1 {
                // Everything Keel started is its child, and `exit` alone reparents all of it to
                // init. A dev server nobody can see and nobody can stop is worse than one that
                // never started — and for a long time that sentence was written here while the
                // dev server was the one thing not in this list.
                monitor::stop_all();
                dev::stop_now();
                std::process::exit(0);
            }
        }
    });
}

#[cfg(test)]
mod shutdown_tests {
    /// Everything that owns a child process is stopped when the parent dies.
    ///
    /// `watch_parent` is the only cleanup Keel gets: macOS has no `PR_SET_PDEATHSIG`, and
    /// `applicationWillTerminate` runs on a ⌘Q and on nothing else. So a subsystem that spawns
    /// processes and is not named there leaks every one of them to init on every quit, silently,
    /// with nothing left on the machine that knows what they are.
    ///
    /// That is not hypothetical: `dev.rs` was missing from this list for its whole life, directly
    /// under a comment reading "a dev server nobody can see and nobody can stop is worse than one
    /// that never started". Verified before the fix — quitting Keel left the server holding its
    /// port, and only `lsof` could find it.
    ///
    /// Read off the source, because there is no way to observe "was included in a shutdown that
    /// ends in `exit(0)`" from inside the process it kills.
    #[test]
    fn the_parent_death_path_stops_everything_that_owns_a_process() {
        let source = include_str!("main.rs");
        let path = source
            .split("fn watch_parent()")
            .nth(1)
            .expect("watch_parent is where a dying parent is noticed");

        for (module, call) in [
            ("monitor", "monitor::stop_all()"),
            ("dev", "dev::stop_now()"),
        ] {
            assert!(
                path.contains(call),
                "`{module}` spawns child processes and `watch_parent` does not stop it: add \
                 `{call}`. Every process it owns is otherwise reparented to init on quit."
            );
        }
    }
}
