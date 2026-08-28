//! Installing and authenticating the CLIs Keel drives.
//!
//! Asking someone to mint a long-lived token when a first-party CLI exists is friction for its own
//! sake, and pasting that token into a text box is the worse security posture of the two. So Keel
//! drives the real tools: check whether they are present, install them, then run their own login
//! flow — `gh` and `wrangler` both use a browser round-trip, and `aws` uses SSO.
//!
//! Both long-running steps stream their output, because these commands print things the user has to
//! act on: `gh auth login --web` shows a one-time code to type into the browser. Swallowing that
//! would leave someone staring at a spinner with no idea what is being asked of them.

use axum::extract::Query;
use axum::response::sse::{Event, Sse};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_stream::wrappers::ReceiverStream;

/// One CLI Keel knows how to set up.
struct Tool {
    id: &'static str,
    label: &'static str,
    binary: &'static str,
    /// Args that print a version, used to detect presence.
    version: &'static [&'static str],
    /// Args that exit zero only when the tool is authenticated.
    whoami: &'static [&'static str],
    /// How to install it, per package manager. First match on the machine wins.
    install: &'static [(&'static str, &'static [&'static str])],
    /// The tool's own login flow.
    login: &'static [&'static str],
    /// Shown when Keel cannot install it here.
    manual: &'static str,
}

const TOOLS: &[Tool] = &[
    Tool {
        id: "gh",
        label: "GitHub CLI",
        binary: "gh",
        version: &["--version"],
        whoami: &["auth", "status"],
        install: &[("brew", &["install", "gh"])],
        // --web is the device flow; https keeps the credential usable for cloning with no SSH key.
        login: &["auth", "login", "--web", "--git-protocol", "https", "--hostname", "github.com"],
        manual: "https://github.com/cli/cli#installation",
    },
    Tool {
        id: "wrangler",
        label: "Wrangler",
        binary: "wrangler",
        version: &["--version"],
        whoami: &["whoami"],
        install: &[
            ("bun", &["add", "--global", "wrangler"]),
            ("npm", &["install", "--global", "wrangler"]),
        ],
        login: &["login"],
        manual: "https://developers.cloudflare.com/workers/wrangler/install-and-update/",
    },
    Tool {
        id: "aws",
        label: "AWS CLI",
        binary: "aws",
        version: &["--version"],
        whoami: &["sts", "get-caller-identity"],
        install: &[("brew", &["install", "awscli"])],
        // SSO is the only AWS login that does not involve pasting a long-lived key. If the profile
        // has no SSO configured this fails loudly, which is the honest outcome.
        login: &["sso", "login"],
        manual: "https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html",
    },
];

fn tool(id: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.id == id)
}

fn exists(binary: &str) -> bool {
    std::process::Command::new(binary)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[derive(Serialize)]
pub struct ToolStatus {
    pub id: &'static str,
    pub label: &'static str,
    pub installed: bool,
    pub version: Option<String>,
    pub authenticated: bool,
    /// The command Keel would run, so the UI can name it before anything happens.
    pub install_cmd: Option<String>,
    pub manual: &'static str,
}

/// Status of every CLI Keel knows about.
pub async fn status() -> axum::Json<Vec<ToolStatus>> {
    let mut out = Vec::new();
    for t in TOOLS {
        let version = std::process::Command::new(t.binary)
            .args(t.version)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .map(|l| l.trim().to_owned())
            });

        let authenticated = version.is_some()
            && std::process::Command::new(t.binary)
                .args(t.whoami)
                .output()
                .is_ok_and(|o| o.status.success());

        // Only offer an installer that actually exists here. Guessing between apt, dnf, pacman and
        // winget is not worth the blast radius of running a package manager on someone's machine.
        let install_cmd = t
            .install
            .iter()
            .find(|(mgr, _)| exists(mgr))
            .map(|(mgr, args)| format!("{mgr} {}", args.join(" ")));

        out.push(ToolStatus {
            id: t.id,
            label: t.label,
            installed: version.is_some(),
            version,
            authenticated,
            install_cmd,
            manual: t.manual,
        });
    }
    axum::Json(out)
}

#[derive(Deserialize)]
pub struct ToolQuery {
    pub id: String,
}

/// Stream a command's combined output to the browser.
fn stream(mut command: Command) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Ok(Event::default().event("fatal").data(e.to_string()))).await;
                return;
            }
        };
        // These tools print the part you must act on to stderr and progress to stdout. Merge both,
        // rather than picking one and losing half the instructions.
        let (out, err) = (child.stdout.take(), child.stderr.take());
        tokio::join!(pump(out, tx.clone()), pump(err, tx.clone()));

        let code = child.wait().await.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        let _ = tx.send(Ok(Event::default().event("done").data(code.to_string()))).await;
    });

    Sse::new(ReceiverStream::new(rx))
}

/// Forward every line of one pipe as an SSE event.
///
/// Generic over the pipe type so stdout and stderr can share it — a closure would monomorphise to
/// whichever was passed first.
async fn pump<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    tx: tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    let Some(pipe) = pipe else { return };
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send(Ok(Event::default().event("line").data(line))).await.is_err() {
            return;
        }
    }
}

fn refuse(message: &str) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let message = message.to_string();
    tokio::spawn(async move {
        let _ = tx.send(Ok(Event::default().event("fatal").data(message))).await;
    });
    Sse::new(ReceiverStream::new(rx))
}

pub async fn install(Query(q): Query<ToolQuery>) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let Some(t) = tool(&q.id) else {
        return refuse("unknown tool");
    };
    let Some((mgr, args)) = t.install.iter().find(|(mgr, _)| exists(mgr)) else {
        return refuse(&format!("no supported package manager found — see {}", t.manual));
    };
    let mut c = Command::new(mgr);
    c.args(*args);
    stream(c)
}

pub async fn login(Query(q): Query<ToolQuery>) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let Some(t) = tool(&q.id) else {
        return refuse("unknown tool");
    };
    if !exists(t.binary) {
        return refuse(&format!("{} is not installed yet", t.label));
    }
    let mut c = Command::new(t.binary);
    c.args(t.login);
    stream(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_an_install_and_a_login_path() {
        for t in TOOLS {
            assert!(!t.install.is_empty(), "{} has no installer", t.id);
            assert!(!t.login.is_empty(), "{} has no login flow", t.id);
            assert!(t.manual.starts_with("https://"), "{} needs a fallback link", t.id);
        }
    }

    #[test]
    fn tool_ids_are_unique_and_resolvable() {
        for t in TOOLS {
            assert_eq!(tool(t.id).map(|x| x.id), Some(t.id));
        }
        assert!(tool("definitely-not-a-tool").is_none());
    }
}
