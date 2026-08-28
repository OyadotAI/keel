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

use axum::Json;
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
    /// The tool's own login flow. `{profile}` is substituted when a profile is supplied.
    login: &'static [&'static str],
    /// Args that print who you are signed in as, for display.
    identity: &'static [&'static str],
    /// Shown when Keel cannot install it here.
    manual: &'static str,
}

/// What Keel knows about the `claude` binary it drives.
///
/// Not one of [`TOOLS`]: those all answer "who am I" by being run, and `claude` has no such
/// subcommand — asking it would cost a real request against the user's own subscription just to
/// draw a checkmark. Its authentication state is on disk instead, so it is read.
#[derive(Serialize, Default)]
pub struct ClaudeStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub authenticated: bool,
    /// The account signed in, for display. Never a token.
    pub account: Option<String>,
    pub plan: Option<String>,
}

pub async fn claude_status() -> Json<ClaudeStatus> {
    let mut out = ClaudeStatus::default();

    if let Ok(v) = std::process::Command::new("claude").arg("--version").output()
        && v.status.success()
    {
        out.installed = true;
        out.version = Some(String::from_utf8_lossy(&v.stdout).trim().to_string());
    }

    // `~/.claude.json` holds the signed-in account. Reading it is free and changes nothing; the
    // credential itself lives in the keychain and Keel never touches it — asking would raise a
    // system prompt for a secret it has no use for.
    let Some(home) = std::env::var("HOME").ok() else {
        return Json(out);
    };
    let Ok(body) = std::fs::read_to_string(format!("{home}/.claude.json")) else {
        return Json(out);
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) else {
        return Json(out);
    };

    if let Some(account) = json.get("oauthAccount") {
        out.authenticated = account.get("emailAddress").and_then(|v| v.as_str()).is_some();
        out.account = account
            .get("emailAddress")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        out.plan = account
            .get("organizationName")
            .and_then(|v| v.as_str())
            .map(str::to_string);
    }

    Json(out)
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
        login: &[
            "auth",
            "login",
            "--web",
            "--git-protocol",
            "https",
            "--hostname",
            "github.com",
        ],
        identity: &["api", "user", "--jq", ".login"],
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
        identity: &["whoami"],
        manual: "https://developers.cloudflare.com/workers/wrangler/install-and-update/",
    },
    Tool {
        id: "aws",
        label: "AWS CLI",
        binary: "aws",
        version: &["--version"],
        whoami: &["sts", "get-caller-identity"],
        install: &[("brew", &["install", "awscli"])],
        // SSO is the only AWS login that does not involve pasting a long-lived key — but it only
        // works against a profile that already has SSO configured. Configuring one is interactive
        // and cannot be driven from here, so Keel detects profiles and says so rather than running
        // a command that will fail.
        login: &["sso", "login", "--profile", "{profile}"],
        identity: &[
            "sts",
            "get-caller-identity",
            "--query",
            "Arn",
            "--output",
            "text",
        ],
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
    /// Who you are signed in as, when the tool can say.
    pub identity: Option<String>,
    /// Named profiles this tool can log into. Only AWS has them.
    pub profiles: Vec<String>,
    /// The command Keel would run, so the UI can name it before anything happens.
    pub install_cmd: Option<String>,
    /// Set when the tool is installed but cannot be logged in from here, with the reason.
    pub blocked: Option<String>,
    pub manual: &'static str,
}

/// Reduce a tool's identity output to the one line worth showing.
///
/// `wrangler whoami` prints a banner, a permissions list and a box-drawn table of accounts. Showing
/// all of it in a settings row would be absurd, so each tool gets the smallest true answer.
fn condense(id: &str, raw: &str) -> String {
    match id {
        "wrangler" => {
            let email = raw
                .lines()
                .find_map(|l| l.split("associated with the email").nth(1))
                .map(|e| e.trim().trim_end_matches('.').to_string());
            // The account name sits in the middle column of a box-drawn table.
            let account = raw
                .lines()
                .filter(|l| l.starts_with('│'))
                .map(|l| {
                    l.trim_matches('│')
                        .split('│')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string()
                })
                .find(|c| !c.is_empty() && c != "Account Name");
            match (email, account) {
                (Some(e), Some(a)) => format!("{e} · {a}"),
                (Some(e), None) => e,
                (None, Some(a)) => a,
                _ => String::new(),
            }
        }
        // An ARN is long; the tail identifies the principal and that is what matters.
        "aws" => raw.trim().rsplit('/').next().unwrap_or("").to_string(),
        _ => raw.trim().lines().next().unwrap_or("").to_string(),
    }
}

/// Named profiles in `~/.aws/config`.
///
/// `aws sso login` needs one, and `aws configure sso` — the command that creates one — is an
/// interactive prompt that cannot be driven from a streamed subprocess. So Keel reads what already
/// exists and tells the user plainly when there is nothing to log into.
fn aws_profiles() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let path = std::path::Path::new(&home).join(".aws").join("config");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            // `[default]` and `[profile name]`; `[sso-session name]` is not a profile.
            if rest == "default" {
                out.push("default".to_string());
            } else if let Some(name) = rest.strip_prefix("profile ") {
                out.push(name.trim().to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
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

        let identity = authenticated
            .then(|| {
                std::process::Command::new(t.binary)
                    .args(t.identity)
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| condense(t.id, &String::from_utf8_lossy(&o.stdout)))
                    .filter(|s| !s.is_empty())
            })
            .flatten();

        let profiles = if t.id == "aws" {
            aws_profiles()
        } else {
            Vec::new()
        };
        let blocked = (t.id == "aws" && version.is_some() && profiles.is_empty()).then(|| {
            "No AWS profile is configured. Run `aws configure sso` in a terminal — it is an \
             interactive prompt Keel cannot drive — then reload."
                .to_string()
        });

        out.push(ToolStatus {
            id: t.id,
            label: t.label,
            installed: version.is_some(),
            version,
            authenticated,
            identity,
            profiles,
            install_cmd,
            blocked,
            manual: t.manual,
        });
    }
    axum::Json(out)
}

#[derive(Deserialize)]
pub struct ToolQuery {
    pub id: String,
    /// Which named profile to log into. Only AWS uses it.
    pub profile: Option<String>,
}

/// Stream a command's combined output to the browser.
fn stream(mut command: Command) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send(Ok(Event::default().event("fatal").data(e.to_string())))
                    .await;
                return;
            }
        };
        // These tools print the part you must act on to stderr and progress to stdout. Merge both,
        // rather than picking one and losing half the instructions.
        let (out, err) = (child.stdout.take(), child.stderr.take());
        tokio::join!(pump(out, tx.clone()), pump(err, tx.clone()));

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

/// Forward every line of one pipe as an SSE event.
///
/// Generic over the pipe type so stdout and stderr can share it — a closure would monomorphise to
/// whichever was passed first.
pub(crate) async fn pump<R: tokio::io::AsyncRead + Unpin>(
    pipe: Option<R>,
    tx: tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    let Some(pipe) = pipe else { return };
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx
            .send(Ok(Event::default().event("line").data(line)))
            .await
            .is_err()
        {
            return;
        }
    }
}

fn refuse(message: &str) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let message = message.to_string();
    tokio::spawn(async move {
        let _ = tx
            .send(Ok(Event::default().event("fatal").data(message)))
            .await;
    });
    Sse::new(ReceiverStream::new(rx))
}

pub async fn install(Query(q): Query<ToolQuery>) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let Some(t) = tool(&q.id) else {
        return refuse("unknown tool");
    };
    let Some((mgr, args)) = t.install.iter().find(|(mgr, _)| exists(mgr)) else {
        return refuse(&format!(
            "no supported package manager found — see {}",
            t.manual
        ));
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

    // Substitute the profile, and refuse rather than run a command that is certain to fail.
    let mut args: Vec<String> = Vec::new();
    for a in t.login {
        if *a == "{profile}" {
            let Some(p) = q.profile.as_deref().filter(|p| !p.is_empty()) else {
                return refuse("pick a profile first");
            };
            if !p
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            {
                return refuse("that profile name is not valid");
            }
            args.push(p.to_string());
        } else {
            args.push((*a).to_string());
        }
    }

    let mut c = Command::new(t.binary);
    c.args(&args);
    stream(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn condenses_wrangler_whoami_to_one_line() {
        let raw = "⛅️ wrangler 4.1\n\
                   👋 You are logged in with an OAuth Token, associated with the email me@example.com.\n\
                   ┌───────────────┬──────────┐\n\
                   │ Account Name  │ Account ID │\n\
                   │ My Account    │ abc123     │\n\
                   └───────────────┴──────────┘\n\
                   🔓 Token Permissions:";
        assert_eq!(condense("wrangler", raw), "me@example.com · My Account");
    }

    #[test]
    fn condenses_an_aws_arn_to_the_principal() {
        assert_eq!(
            condense("aws", "arn:aws:sts::12345:assumed-role/AdminRole/mk\n"),
            "mk"
        );
    }

    #[test]
    fn aws_login_refuses_without_a_profile() {
        // The placeholder must survive into the arg list, so login() has something to reject on.
        let aws = tool("aws").expect("aws is registered");
        assert!(aws.login.contains(&"{profile}"));
    }

    #[test]
    fn every_tool_has_an_install_and_a_login_path() {
        for t in TOOLS {
            assert!(!t.install.is_empty(), "{} has no installer", t.id);
            assert!(!t.login.is_empty(), "{} has no login flow", t.id);
            assert!(
                t.manual.starts_with("https://"),
                "{} needs a fallback link",
                t.id
            );
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
