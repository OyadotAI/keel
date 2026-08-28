//! Keel — the local IDE that makes a repo shippable.
//!
//! At Milestone 0 only `keel scan` is wired end to end. That is deliberate: the readiness scan is
//! useful with nothing connected, so it ships and is trusted before Keel is ever handed a cloud
//! credential.

mod agents;
mod approve;
mod api;
mod clitools;
mod connect;
mod dev;
mod fsops;
mod gui;
mod mcp;
mod permissions;
mod prefs;
mod plugins;
mod project;
mod render;
mod serve;
mod term;
mod verify;

use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use keel_harness::quarantine;
use keel_scanner::{RepoContext, scan};
use keel_workspace::Workspace;

#[derive(Parser)]
#[command(name = "keel", version, about = "Make a repo shippable")]
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

    /// Open the Keel IDE in a browser.
    Serve {
        /// Repository to open. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: Utf8PathBuf,

        #[arg(long, default_value_t = 7777)]
        port: u16,

        /// Do not open a browser window.
        #[arg(long)]
        no_open: bool,
    },

    /// Open Keel as an application, in its own window.
    ///
    /// What the macOS bundle runs, and what a bare `keel` does. Launched from the Dock there is no
    /// working directory worth inferring a project from, so the last one is reopened — and on a
    /// first run, the welcome screen is shown instead.
    App {
        #[arg(long, default_value_t = 7777)]
        port: u16,

        /// Serve without a window, and open a browser tab instead. For a machine with no display.
        #[arg(long)]
        headless: bool,
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

    // macOS hands a bundled process a `-psn_0_…` serial number on some launches. It is not an
    // argument anyone typed, and clap would reject it and take the application down on start.
    let args = std::env::args_os().filter(|a| !a.to_string_lossy().starts_with("-psn_"));
    let cli = Cli::parse_from(args);

    match cli.command.unwrap_or(Command::App {
        port: 7777,
        headless: false,
    }) {
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
            no_open,
        } => {
            let repo = path
                .canonicalize_utf8()
                .with_context(|| format!("resolving {path}"))?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("starting the async runtime")?
                .block_on(serve::run(repo, port, !no_open))?;
        }

        Command::Approve { port } => {
            // Every failure here prints nothing and exits 0, which defers to Claude Code's own
            // permission check. A guardrail that can wedge the agent is one people disable.
            let mut raw = String::new();
            if std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw).is_err() {
                return Ok(());
            }
            let Ok(hook) = serde_json::from_str::<approve::HookInput>(&raw) else {
                return Ok(());
            };

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

        Command::App { port, headless } => {
            if headless {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .context("starting the async runtime")?
                    .block_on(serve::run_app(port, true))?;
            } else {
                // Takes over this thread and never returns: AppKit's run loop has to be the main
                // one, so the server is what moves to a thread, not the window.
                gui::run(port)?;
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
