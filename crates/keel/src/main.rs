//! Keel — the local IDE that makes a repo shippable.
//!
//! At Milestone 0 only `keel scan` is wired end to end. That is deliberate: the readiness scan is
//! useful with nothing connected, so it ships and is trusted before Keel is ever handed a cloud
//! credential.

mod api;
mod connect;
mod render;
mod serve;

use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use keel_harness::quarantine;
use keel_scanner::{RepoContext, scan};
use keel_workspace::Workspace;

#[derive(Parser)]
#[command(name = "keel", version, about = "Make a repo shippable")]
struct Cli {
    #[command(subcommand)]
    command: Command,
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

    let cli = Cli::parse();

    match cli.command {
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

        Command::Serve { path, port, no_open } => {
            let repo = path
                .canonicalize_utf8()
                .with_context(|| format!("resolving {path}"))?;
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("starting the async runtime")?
                .block_on(serve::run(repo, port, !no_open))?;
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
