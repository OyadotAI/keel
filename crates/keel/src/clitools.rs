//! Installing and authenticating the CLIs Keel drives.
//!
//! Asking someone to mint a long-lived token when a first-party CLI exists is friction for its own
//! sake, and pasting that token into a text box is the worse security posture of the two. So Keel
//! drives the real tools: check whether they are present, install them, then run their own login
//! flow — `gh`, `wrangler` and `gcloud` all use a browser round-trip. `kubectl` and `docker`
//! have none, and Keel says where their credentials actually come from instead.
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
use std::sync::OnceLock;
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
    /// A command to hand the terminal when this tool cannot be connected from the UI.
    ///
    /// Keel owns a real shell, so an interactive flow it cannot *drive* it can still *host*. The
    /// alternative is a sentence telling someone to go and run something somewhere else, which is
    /// what the first version of this did for kubectl and docker.
    setup: &'static str,
}

/// What Keel knows about the `claude` binary it drives.
///
/// Authentication is queried through `claude auth status`, which does not start an agent turn.
/// A cached account in .claude.json is not proof that its credentials are still valid.
#[derive(Serialize, Default)]
pub struct ClaudeStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub authenticated: bool,
    /// The account signed in, for display. Never a token.
    pub account: Option<String>,
    pub plan: Option<String>,
    /// Whether Homebrew exists, which every other install path depends on.
    pub brew: bool,
    /// The command that installs it, for the terminal.
    pub brew_install: &'static str,
}

/// Install Claude Code.
///
/// The vendor's own installer, which is native: it needs no npm and no Node, and it refuses to run
/// under sudo because everything it writes goes under `$HOME`. Both of those are why this can be
/// streamed like any other command rather than handed to the terminal — there is no password
/// prompt to get stuck behind.
///
/// A fresh machine has none of brew, npm or claude, and telling somebody to go and install three
/// things before the application will do anything is not an onboarding flow. This is the one that
/// matters: without `claude` there is no agent, and Keel is a window over a scanner.
pub async fn install_claude() -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    // Echoed before it runs. Piping a downloaded script into a shell is a reasonable thing to do
    // with a vendor's own installer and an unreasonable thing to do invisibly.
    const COMMAND: &str = "echo '$ curl -fsSL https://claude.ai/install.sh | bash'; \
                           curl -fsSL https://claude.ai/install.sh | bash";

    let mut command = Command::new("bash");
    // A login shell, so the PATH the installer writes into is the one a terminal would read.
    command.args(["-o", "pipefail", "-lc", COMMAND]);
    stream(command)
}

pub async fn login_claude() -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let mut command = Command::new("claude");
    command.args(["auth", "login"]);
    stream(command)
}

/// Whether Homebrew is here, and how to get it.
///
/// Every `install_cmd` in [`TOOLS`] except wrangler's goes through brew, so on a fresh Mac the
/// first missing thing is brew itself and every install button is dead until it exists. Its
/// installer wants sudo, so Keel hosts it in the terminal rather than streaming it into a console
/// with no way to answer a password prompt.
pub const BREW_INSTALL: &str = "/bin/bash -c \"$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\"";

pub async fn claude_status() -> Json<ClaudeStatus> {
    let mut out = ClaudeStatus {
        brew: run("brew", &["--version"]).await.is_some(),
        brew_install: BREW_INSTALL,
        ..Default::default()
    };

    if let Some(version) = run("claude", &["--version"]).await {
        out.installed = true;
        out.version = Some(version.trim().to_string());
    }
    if out.installed
        && let Some(body) = run("claude", &["auth", "status"]).await
    {
        apply_claude_auth(&mut out, &body);
    }
    Json(out)
}

fn apply_claude_auth(out: &mut ClaudeStatus, body: &str) {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return;
    };
    out.authenticated = json.get("loggedIn").and_then(|v| v.as_bool()) == Some(true);
    if out.authenticated {
        out.account = json
            .get("email")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        out.plan = json
            .get("subscriptionType")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
    }
}

const TOOLS: &[Tool] = &[
    Tool {
        id: "gh",
        label: "GitHub CLI",
        binary: "gh",
        version: &["--version"],
        whoami: &["auth", "status", "--active", "--hostname", "github.com"],
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
        identity: &["api", "user", "--hostname", "github.com", "--jq", ".login"],
        manual: "https://github.com/cli/cli#installation",
        setup: "",
    },
    Tool {
        id: "wrangler",
        label: "Wrangler",
        binary: "wrangler",
        version: &["--version"],
        whoami: &["whoami", "--json"],
        // brew first, and its formula declares node as a dependency, so a bare machine gets both
        // from one command. bun and npm stay as fallbacks for anyone who has a runtime but not
        // Homebrew.
        install: &[
            ("brew", &["install", "cloudflare-wrangler"]),
            ("bun", &["add", "--global", "wrangler"]),
            ("npm", &["install", "--global", "wrangler"]),
        ],
        login: &["login"],
        identity: &["whoami", "--json"],
        manual: "https://developers.cloudflare.com/workers/wrangler/install-and-update/",
        setup: "",
    },
    Tool {
        id: "gcloud",
        label: "Google Cloud",
        binary: "gcloud",
        version: &["--version"],
        whoami: &["auth", "print-access-token"],
        install: &[("brew", &["install", "--cask", "google-cloud-sdk"])],
        // Opens a browser and waits. That is fine here: it is the user's own machine and their own
        // Google account, and there is no paste-a-key alternative worth offering instead.
        login: &["auth", "login", "--update-adc"],
        identity: &[
            "auth",
            "list",
            "--filter=status:ACTIVE",
            "--format=value(account)",
        ],
        manual: "https://cloud.google.com/sdk/docs/install",
        setup: "",
    },
    Tool {
        id: "kubectl",
        label: "Kubernetes",
        binary: "kubectl",
        // Not `-o=yaml`: its first line is the literal word "clientVersion:", and the first line
        // is what gets shown. Plain `--client` prints "Client Version: v1.35.0".
        version: &["version", "--client=true"],
        // A cluster you cannot reach is not a connection, so this asks the API server rather than
        // reading kubeconfig — a context can exist and point at nothing.
        whoami: &["cluster-info"],
        install: &[("brew", &["install", "kubectl"])],
        // There is no `kubectl login`. Credentials come from the provider — `gcloud container
        // clusters get-credentials`, `aws eks update-kubeconfig`, or a file someone handed you —
        // so Keel reports the current context and does not pretend it can sign you in.
        login: &[],
        identity: &["config", "current-context"],
        manual: "https://kubernetes.io/docs/tasks/tools/",
        // Settings has a native chooser; a printed table has no selection action.
        setup: "",
    },
    Tool {
        id: "docker",
        label: "Docker",
        binary: "docker",
        version: &["--version"],
        // `docker info` fails when the daemon is not running, which is the condition worth
        // reporting: the binary being installed says nothing about whether anything can run.
        whoami: &["info", "--format", "{{.ServerVersion}}"],
        install: &[("brew", &["install", "--cask", "docker"])],
        login: &[],
        identity: &["info", "--format", "{{.Name}} · {{.ServerVersion}}"],
        manual: "https://docs.docker.com/get-docker/",
        setup: "open -a Docker",
    },
    Tool {
        id: "aws",
        label: "AWS",
        binary: "aws",
        version: &["--version"],
        whoami: &["sts", "get-caller-identity"],
        install: &[("brew", &["install", "awscli"])],
        // Only works against a profile that already exists. Keel writes one for Identity Center
        // (see `aws::configure_sso`) and detects one that was set up by hand; what it never does
        // is ask for an access key, which is the only other way in and the wrong thing to type
        // into an IDE.
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
        // `aws configure` prompts for a key and a secret. Keel cannot drive it and should not
        // reimplement it — it hosts it, and the key goes to the CLI's stdin rather than through
        // any part of Keel.
        setup: "aws configure",
    },
    Tool {
        id: "tailscale",
        label: "Tailscale",
        binary: "tailscale",
        version: &["version"],
        // Bare `status`, and deliberately not `status --json` or `ip -4`. Measured on a machine
        // with Tailscale installed but stopped: both of those exit 0 — `ip -4` prints
        // "no current Tailscale IPs" to stderr and still succeeds, and `--json` happily returns a
        // document saying `"BackendState": "Stopped"`. Only bare `status` exits 1. Either of the
        // other two would have drawn a checkmark next to a VPN that was carrying nothing, which is
        // the one thing this field exists to prevent.
        whoami: &["status"],
        // The cask, not the formula. `brew install tailscale` is the daemon on its own and needs
        // `sudo brew services start tailscale` before it does anything; the app installs the CLI
        // at /usr/local/bin/tailscale and manages the daemon itself. The cask is named
        // `tailscale-app` — plain `tailscale` is an alias that resolves to it and may not always.
        install: &[("brew", &["install", "--cask", "tailscale-app"])],
        // `tailscale up` prints a URL and then waits, and on macOS it can ask for rights Keel
        // cannot grant it. Hosted in the terminal rather than driven, for the same reason
        // `aws configure` is.
        login: &[],
        // The tailnet address, which is exactly what pairing needs to show. Only ever read once
        // `whoami` has said the tunnel is actually up, so the empty case never reaches display.
        identity: &["ip", "-4"],
        manual: "https://tailscale.com/download/macos",
        setup: "tailscale up",
    },
];

/// Profiles the AWS CLI can see, across both `~/.aws/config` and `~/.aws/credentials`.
///
/// `aws configure list-profiles` rather than parsing the INI: it sees SSO profiles in `config` and
/// key-based ones in `credentials`, and it is the CLI's own answer to the question rather than
/// Keel's guess at it.
async fn aws_profiles() -> Vec<String> {
    run("aws", &["configure", "list-profiles"])
        .await
        .map(|o| {
            o.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn tool(id: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.id == id)
}

/// Whether a binary is there and runnable.
///
/// Asks it the way its own [`Tool`] says to, rather than assuming `--version`. `kubectl` rejects
/// that flag outright — it wants `version --client` — so the generic form reported it as missing
/// on a machine where it was installed, and the sign-in path refused with "not installed yet".
fn exists(binary: &str) -> bool {
    let args: &[&str] = TOOLS
        .iter()
        .find(|t| t.binary == binary)
        .map(|t| t.version)
        // A package manager, which is the other caller here, and those all take `--version`.
        .unwrap_or(&["--version"]);

    std::process::Command::new(binary)
        .args(args)
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
    /// Named profiles this tool can log into. Nothing uses them since AWS was dropped; kept so the
    /// UI's shape does not change under it, and because a provider with profiles will come back.
    pub profiles: Vec<String>,
    /// The command Keel would run, so the UI can name it before anything happens.
    pub install_cmd: Option<String>,
    /// A command Keel will type into its own terminal, where an interactive flow works.
    pub setup: Option<String>,
    /// Set when the tool is installed but cannot be logged in from here, with the reason.
    pub blocked: Option<String>,
    /// Provider whose browser login can recover this connection failure.
    pub reconnect: Option<&'static str>,
    pub manual: &'static str,
}

/// Reduce a tool's identity output to the one line worth showing.
///
/// `wrangler whoami` prints a banner, a permissions list and a box-drawn table of accounts. Showing
/// all of it in a settings row would be absurd, so each tool gets the smallest true answer.
fn condense(id: &str, raw: &str) -> String {
    match id {
        "wrangler" => {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) {
                let email = json.get("email").and_then(|v| v.as_str());
                let account = json
                    .get("accounts")
                    .and_then(|v| v.as_array())
                    .and_then(|v| v.first())
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str());
                return [email, account]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
            }
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
        // `cluster-info` prints a banner; the first line names the control plane, which is the
        // one fact worth showing about a cluster you are pointed at.
        "kubectl" => raw
            .trim()
            .lines()
            .next()
            .unwrap_or("")
            .replace("Kubernetes control plane", "")
            .replace("is running at", "")
            .trim()
            .to_string(),
        _ => raw.trim().lines().next().unwrap_or("").to_string(),
    }
}

/// Install everything that is missing, in one go.
///
/// Two things are prerequisites and cannot be done from here: `claude`, because Keel has nothing
/// to drive without it, and Homebrew, because its installer wants a password. Everything else is a
/// `brew install` and there is no reason to make somebody run six of them by hand, one at a time,
/// discovering each missing tool only when a button for it fails.
///
/// Sequential rather than parallel: brew serialises its own work anyway, and interleaved output
/// from six installs is not something anyone can read.
pub async fn install_all() -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        let say = |line: String| {
            let tx = tx.clone();
            async move {
                let _ = tx.send(Ok(Event::default().event("line").data(line))).await;
            }
        };

        if !exists("brew") {
            say("Homebrew is missing, and everything here needs it.".into()).await;
            say("Install it first — Keel can open a terminal with the command ready.".into()).await;
            let _ = tx.send(Ok(Event::default().event("done").data("1"))).await;
            return;
        }

        let missing: Vec<&Tool> = TOOLS.iter().filter(|t| !exists(t.binary)).collect();
        if missing.is_empty() {
            say("Everything is already installed.".into()).await;
            let _ = tx.send(Ok(Event::default().event("done").data("0"))).await;
            return;
        }

        say(format!(
            "Installing {}.\n",
            missing
                .iter()
                .map(|t| t.label)
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .await;

        let mut failed = 0;
        for t in missing {
            // No special cases. wrangler used to need node installed first, because it came from
            // a JavaScript registry; the brew formula declares node as a dependency, so brew does
            // that itself and the step here was code that only looked like it was doing something.
            let Some((mgr, args)) = t.install.iter().find(|(mgr, _)| exists(mgr)) else {
                say(format!(
                    "{} has no installer here — see {}",
                    t.label, t.manual
                ))
                .await;
                failed += 1;
                continue;
            };

            say(format!("\n$ {mgr} {}", args.join(" "))).await;
            let mut command = Command::new(mgr);
            command.args(*args);
            if run_install(&mut command, &tx).await != 0 {
                failed += 1;
            }
        }

        say(if failed == 0 {
            "\nDone. Reload connections to see them.".into()
        } else {
            format!("\n{failed} did not install. The output above says why.")
        })
        .await;
        let _ = tx
            .send(Ok(Event::default().event("done").data(failed.to_string())))
            .await;
    });

    Sse::new(ReceiverStream::new(rx))
}

/// Run one install, forwarding its output, and give back its exit code.
async fn run_install(
    command: &mut Command,
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) -> i32 {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let Ok(mut child) = command.spawn() else {
        return 1;
    };
    let (out, err) = (child.stdout.take(), child.stderr.take());
    tokio::join!(pump(out, tx.clone()), pump(err, tx.clone()));
    child
        .wait()
        .await
        .map(|s| s.code().unwrap_or(-1))
        .unwrap_or(-1)
}

/// What is on this machine, for the agent's system prompt.
///
/// Returns (package manager, present, absent). The agent cannot see any of this: it discovers a
/// missing tool by running something that fails, and then either works around the gap or gives up
/// — both of which are worse than installing it, which it will not think to do if it does not know
/// there is a package manager.
pub fn toolchain() -> (Option<&'static str>, Vec<&'static str>, Vec<&'static str>) {
    // Cached for the life of the daemon. This runs ~17 `--version` processes — measured at 1.1s
    // warm — and it ran on *every turn*, inside the system prompt, before the agent was spawned
    // and so before a single SSE event. That is a blank screen the person reads as "stuck", and
    // the answer cannot change while Keel is running: nobody installs Go mid-session.
    static CACHED: OnceLock<(Option<&'static str>, Vec<&'static str>, Vec<&'static str>)> =
        OnceLock::new();
    CACHED.get_or_init(uncached_toolchain).clone()
}

fn uncached_toolchain() -> (Option<&'static str>, Vec<&'static str>, Vec<&'static str>) {
    let manager = ["brew", "apt-get", "dnf"].into_iter().find(|m| exists(m));

    // Runtimes and version-control the agent reaches for constantly, alongside the CLIs Keel
    // manages. `git` is here because a repository without it changes what the agent can do.
    let extra = ["git", "node", "npm", "bun", "cargo", "go", "python3", "uv"];

    let mut present = Vec::new();
    let mut absent = Vec::new();
    for name in TOOLS.iter().map(|t| t.binary).chain(extra) {
        if exists(name) {
            present.push(name)
        } else {
            absent.push(name)
        }
    }
    (manager, present, absent)
}

/// Status of every CLI Keel knows about.
/// Status of every CLI Keel knows about.
///
/// Concurrently, and with the async process API. Sequentially, with the blocking one, this took
/// 7.4 seconds — six tools, three commands each, several of them round-tripping to a cloud API to
/// answer "who am I". It also blocked the executor for the whole time, so nothing else Keel served
/// could respond either. The connections panel appearing to hang was that, exactly.
/// The tools the `blocked` match below has words for.
///
/// Kept next to that match, and asserted against by `a_tool_without_a_login_is_one_we_explain`: a
/// tool with no login of its own is unreachable from Settings unless something explains why.
#[cfg(test)]
const BLOCKED_EXPLAINS: &[&str] = &["kubectl", "docker", "aws", "tailscale"];

#[cfg(test)]
#[test]
fn kubernetes_setup_does_not_end_at_a_print_only_command() {
    assert!(
        tool("kubectl").unwrap().setup.is_empty(),
        "Kubernetes setup needs a native context chooser, not a terminal table"
    );
}

#[derive(Default, Deserialize)]
pub struct StatusQuery {
    pub aws_profile: Option<String>,
}

pub async fn status(Query(q): Query<StatusQuery>) -> axum::Json<Vec<ToolStatus>> {
    let aws_profile = q.aws_profile.as_deref().filter(|p| !p.is_empty());
    let checks = TOOLS.iter().map(|t| async move {
        // Within one tool the calls are ordered — asking a missing binary who it is wastes a
        // process spawn, and asking an unauthenticated one wastes a network round trip.
        let version = run(t.binary, t.version)
            .await
            .and_then(|o| o.lines().next().map(|l| l.trim().to_owned()));

        let args = scoped_args(t.whoami, if t.id == "aws" { aws_profile } else { None });
        let connection = if version.is_some() {
            run_checked(t.binary, &args).await.and_then(|body| validate_connection(t.id, body))
        } else {
            Err("Tool is not installed.".to_string())
        };
        let authenticated = connection.is_ok();
        let reconnect = (t.id == "kubectl"
            && connection.as_ref().err().is_some_and(|e| google_reauthentication_needed(e)))
            .then_some("gcloud");

        // A selected context exists independently of credentials or cluster reachability.
        let identity = if authenticated || (t.id == "kubectl" && version.is_some()) {
            run(t.binary, &scoped_args(t.identity, if t.id == "aws" { aws_profile } else { None }))
                .await
                .map(|o| condense(t.id, &o))
                .filter(|s| !s.is_empty())
        } else {
            None
        };

        // Only offer an installer that actually exists here. Guessing between apt, dnf, pacman and
        // winget is not worth the blast radius of running a package manager on someone's machine.
        let install_cmd = t
            .install
            .iter()
            .find(|(mgr, _)| exists(mgr))
            .map(|(mgr, args)| format!("{mgr} {}", args.join(" ")));

        let profiles = if t.id == "aws" {
            aws_profiles().await
        } else {
            Vec::new()
        };

        // Why it is not connected, in its own words. A tool with no login of its own takes its
        // credentials from somewhere else, and naming that is more use than a button that runs
        // nothing — but better still is a command, which the UI can hand to the terminal.
        let blocked = match t.id {
            "gcloud" if version.is_some() && !authenticated => Some(format!(
                "Google Cloud connection check failed. Sign in again if your credentials have expired.\n{}",
                connection.as_ref().err().map(String::as_str).unwrap_or("Unknown error")
            )),
            "kubectl" if version.is_some() && !authenticated => {
                Some(kubernetes_connection_message(
                    identity.as_deref(),
                    connection
                        .as_ref()
                        .err()
                        .map(String::as_str)
                        .unwrap_or("Unknown error"),
                ))
            }
            "docker" if version.is_some() && !authenticated => Some(format!(
                "Docker engine check failed. Check the selected Docker context and its connection; start Docker Desktop if you use its local engine.\n{}",
                connection.as_ref().err().map(String::as_str).unwrap_or("Unknown error")
            )),
            "aws" if version.is_some() && profiles.is_empty() => Some(
                "No AWS profile exists yet. Set one up with Identity Center below, or run \
                 `aws configure` for an access key — Keel never asks for one."
                    .to_string(),
            ),
            "aws" if version.is_some() && !authenticated => Some(format!(
                "AWS check failed for {}. Select your profile; renew SSO sign-in if its session expired.\n{}",
                aws_profile.unwrap_or("the default credential chain"),
                connection.as_ref().err().map(String::as_str).unwrap_or("Unknown error")
            )),
            // Installed and stopped is the ordinary state of a VPN, not a failure, so this says
            // what it is rather than reporting it broken. `tailscale up` opens a browser and can
            // ask for rights Keel does not have, which is why it is offered to the terminal as a
            // command rather than as a button that runs nothing.
            "tailscale" if version.is_some() && !authenticated => Some(format!(
                "Tailscale connection check failed. Check the app's connection and permissions.\n{}",
                connection.as_ref().err().map(String::as_str).unwrap_or("Unknown error")
            )),
            _ if version.is_some() && !authenticated => connection.as_ref().err().cloned(),
            _ => None,
        };

        // Only offered when it is the thing to do: installed, not working, and with something
        // interactive behind it.
        let setup = (!t.setup.is_empty() && version.is_some() && !authenticated)
            .then(|| t.setup.to_string());

        ToolStatus {
            id: t.id,
            label: t.label,
            installed: version.is_some(),
            version,
            authenticated,
            identity,
            profiles,
            install_cmd,
            setup,
            blocked,
            reconnect,
            manual: t.manual,
        }
    });

    let mut statuses = futures_util::future::join_all(checks).await;
    if statuses.iter().any(|t| t.reconnect == Some("gcloud"))
        && let Some(cloud) = statuses.iter_mut().find(|t| t.id == "gcloud")
    {
        cloud.authenticated = false;
        cloud.blocked = Some("Your GKE connection requires Google Cloud reauthentication. Sign in again, then test the Kubernetes connection.".to_string());
    }
    axum::Json(statuses)
}

fn scoped_args<'a>(args: &[&'a str], profile: Option<&'a str>) -> Vec<&'a str> {
    let mut out = args.to_vec();
    if let Some(profile) = profile {
        out.extend(["--profile", profile]);
    }
    out
}

fn validate_connection(id: &str, body: String) -> Result<String, String> {
    if id == "wrangler" {
        let json: serde_json::Value = serde_json::from_str(&body).map_err(|_| {
            "Could not read Wrangler authentication status. Update Wrangler and retry.".to_string()
        })?;
        if json.get("loggedIn").and_then(|v| v.as_bool()) != Some(true) {
            return Err(
                "Cloudflare sign-in required. Sign in with Wrangler to connect your account."
                    .to_string(),
            );
        }
    }
    Ok(body)
}

/// Run a command and give back its stdout, or nothing if it failed.
async fn run(program: &str, args: &[&str]) -> Option<String> {
    run_checked(program, args).await.ok()
}

fn kubernetes_connection_message(context: Option<&str>, error: &str) -> String {
    if google_reauthentication_needed(error) {
        return format!(
            "Google Cloud reconnection needed. Renew your Google Cloud sign-in, then Test connection. Your Kubernetes context has not changed.\n{error}"
        );
    }
    if context.is_some() {
        format!(
            "Context selected · connection check failed. Check your VPN or cluster credentials, then Test connection.\n{error}"
        )
    } else {
        format!(
            "No current context could be read. Choose a context to configure cluster access.\n{error}"
        )
    }
}

fn google_reauthentication_needed(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    (e.contains("gcloud")
        || e.contains("gke-gcloud-auth-plugin")
        || e.contains("accounts.google.com"))
        && [
            "invalid_grant",
            "reauth",
            "expired",
            "revoked",
            "gcloud auth login",
            "refresh token",
            "could not refresh access token",
        ]
        .iter()
        .any(|reason| e.contains(reason))
}

#[cfg(test)]
#[test]
fn expired_google_credentials_have_provider_recovery_not_context_setup() {
    let error = "gke-gcloud-auth-plugin: ERROR: (gcloud.config.config-helper) There was a problem refreshing your current auth tokens: invalid_grant: Token has been expired or revoked. Please run gcloud auth login";
    assert!(google_reauthentication_needed(error));
    assert!(
        kubernetes_connection_message(Some("gke_project_cluster"), error)
            .contains("Google Cloud reconnection needed")
    );
    assert!(!google_reauthentication_needed(
        "gke-gcloud-auth-plugin: executable file not found"
    ));
    assert!(!google_reauthentication_needed(
        "gcloud: dial tcp: network is unreachable"
    ));
    assert!(!google_reauthentication_needed(
        "Forbidden: user cannot list namespaces"
    ));
    assert_eq!(
        tool("gcloud").unwrap().whoami,
        &["auth", "print-access-token"]
    );
}

async fn run_checked(program: &str, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(8), command.output())
        .await
        .map_err(|_| "Connection check timed out after 8 seconds.".to_string())?
        .map_err(|e| format!("Could not run {program}: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail: String = stderr.trim().chars().take(1600).collect();
        Err(if detail.is_empty() {
            format!("{program} exited with {}.", output.status)
        } else {
            detail
        })
    }
}

#[cfg(test)]
#[test]
fn selected_context_survives_a_failed_connection_message() {
    let message = kubernetes_connection_message(Some("staging"), "credential plugin missing");
    assert!(message.contains("Context selected · connection check failed"));
    assert!(message.contains("credential plugin missing"));
    assert!(!message.contains("Choose a context"));
    assert!(kubernetes_connection_message(None, "missing config").contains("Choose a context"));
}

/// Only context metadata crosses the API, never kubeconfig credentials or certificates.
#[derive(Debug, Serialize, Deserialize)]
pub struct KubernetesContext {
    pub name: String,
    #[serde(default)]
    pub context: KubernetesTarget,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct KubernetesTarget {
    #[serde(default)]
    pub cluster: String,
    #[serde(default)]
    pub namespace: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct KubernetesConfig {
    #[serde(default, rename = "current-context")]
    pub current: Option<String>,
    // kubectl emits null, rather than [], when a config has no contexts.
    #[serde(default)]
    pub contexts: Option<Vec<KubernetesContext>>,
}

type KubernetesResult = Result<Json<KubernetesConfig>, (axum::http::StatusCode, String)>;

async fn kubernetes_command(program: &std::ffi::OsStr, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(8), command.output())
        .await
        .map_err(|_| "kubectl timed out. Check your configuration and try Refresh.".to_owned())?
        .map_err(|_| {
            "Could not start kubectl. Install Kubernetes in Settings → Tools, then retry."
                .to_owned()
        })?;
    if !output.status.success() {
        return Err(format!(
            "kubectl could not update or read its configuration: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1000)
                .collect::<String>()
                .trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn read_kubernetes(program: &std::ffi::OsStr) -> Result<KubernetesConfig, String> {
    // Local config only. Never --raw, never an API request or credential plugin execution.
    let json = kubernetes_command(program, &["config", "view", "-o", "json"]).await?;
    let mut config: KubernetesConfig = serde_json::from_str(&json).map_err(|_| {
        "kubectl returned an unreadable configuration. Check your kubeconfig and retry.".to_owned()
    })?;
    if let Some(contexts) = &mut config.contexts {
        contexts.sort_by(|a, b| a.name.cmp(&b.name));
        contexts.dedup_by(|a, b| a.name == b.name);
    }
    Ok(config)
}

pub async fn kubernetes_contexts() -> KubernetesResult {
    read_kubernetes(std::ffi::OsStr::new("kubectl"))
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

#[derive(Deserialize)]
pub struct KubernetesSelection {
    pub name: String,
}

async fn select_kubernetes(
    program: &std::ffi::OsStr,
    name: &str,
) -> Result<KubernetesConfig, String> {
    let config = read_kubernetes(program).await?;
    if !config
        .contexts
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|c| c.name == name)
    {
        return Err(
            "That context is no longer available. Refresh and choose another context.".to_owned(),
        );
    }
    // An argv value, not shell text. -- prevents names from becoming kubectl flags.
    kubernetes_command(program, &["config", "use-context", "--", name]).await?;
    let updated = read_kubernetes(program).await?;
    if updated.current.as_deref() != Some(name) {
        return Err(
            "The selected context was not saved. Refresh and check your kubeconfig permissions."
                .to_owned(),
        );
    }
    Ok(updated)
}

pub async fn kubernetes_select(Json(selection): Json<KubernetesSelection>) -> KubernetesResult {
    select_kubernetes(std::ffi::OsStr::new("kubectl"), &selection.name)
        .await
        .map(Json)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))
}

#[cfg(test)]
mod kubernetes_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn stub(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kubectl");
        std::fs::write(
            &path,
            format!("#!/bin/sh\ncd '{}'\n{}\n", dir.path().display(), body),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        (dir, path)
    }

    #[tokio::test]
    async fn connection_check_preserves_provider_auth_errors() {
        let (_dir, path) = stub(
            "printf '%s' 'gcloud: invalid_grant: Token has been expired or revoked' >&2\nexit 1",
        );
        let error = run_checked(path.to_str().unwrap(), &[]).await.unwrap_err();
        assert!(google_reauthentication_needed(&error));
        assert!(error.contains("invalid_grant"));
        assert!(run(path.to_str().unwrap(), &[]).await.is_none());
    }

    #[tokio::test]
    async fn metadata_excludes_credentials_and_accepts_empty_configs() {
        let (_dir, path) = stub(
            r#"printf '%s' '{"current-context":"dev","contexts":[{"name":"dev","context":{"cluster":"local","user":"secret-user","namespace":"apps"}}],"users":[{"user":{"token":"SECRET"}}]}'"#,
        );
        let config = read_kubernetes(path.as_os_str()).await.unwrap();
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("SECRET"));
        assert!(!json.contains("secret-user"));
        assert_eq!(config.current.as_deref(), Some("dev"));
        for json in ["{}", r#"{"contexts":null}"#, r#"{"contexts":[]}"#] {
            let config: KubernetesConfig = serde_json::from_str(json).unwrap();
            assert!(config.contexts.unwrap_or_default().is_empty());
        }
    }

    #[tokio::test]
    async fn context_selection_uses_literal_argv_and_verifies_saved_context() {
        let (dir, path) = stub(
            r#"
if [ "$2" = view ]; then
  if [ -f applied ]; then current='dev; echo injected'; else current=old; fi
  printf '{"current-context":"%s","contexts":[{"name":"dev; echo injected","context":{"cluster":"local"}}]}' "$current"
else
  [ "$1" = config ] && [ "$2" = use-context ] && [ "$3" = -- ] && [ "$4" = 'dev; echo injected' ] && [ "$#" = 4 ] || exit 2
  touch applied
fi"#,
        );
        let config = select_kubernetes(path.as_os_str(), "dev; echo injected")
            .await
            .unwrap();
        assert_eq!(config.current.as_deref(), Some("dev; echo injected"));
        assert!(dir.path().join("applied").exists());
        assert!(!dir.path().join("injected").exists());
    }

    #[tokio::test]
    async fn stale_choices_are_rejected_without_running_use_context() {
        let (dir, path) = stub(
            r#"
if [ "$2" = view ]; then printf '%s' '{"contexts":[]}'; else touch mutated; fi"#,
        );
        let error = select_kubernetes(path.as_os_str(), "missing")
            .await
            .unwrap_err();
        assert!(error.contains("no longer available"));
        assert!(!dir.path().join("mutated").exists());
    }

    #[tokio::test]
    async fn failures_and_false_success_are_visible() {
        let (_dir, path) = stub("echo 'permission denied' >&2; exit 1");
        assert!(
            read_kubernetes(path.as_os_str())
                .await
                .unwrap_err()
                .contains("permission denied")
        );
        let (_dir, path) =
            stub(r#"printf '%s' '{"current-context":"old","contexts":[{"name":"dev"}]}'"#);
        assert!(
            select_kubernetes(path.as_os_str(), "dev")
                .await
                .unwrap_err()
                .contains("not saved")
        );
        let (_dir, path) = stub("echo not-json");
        assert!(
            read_kubernetes(path.as_os_str())
                .await
                .unwrap_err()
                .contains("unreadable")
        );
    }
}

#[derive(Deserialize)]
pub struct ToolQuery {
    pub id: String,
    /// Which named profile to log into. Unused since AWS was dropped.
    pub profile: Option<String>,
}

/// Stream a command's combined output to the browser.
fn stream(mut command: Command) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);
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
        tokio::select! {
            _ = async { tokio::join!(pump(out, tx.clone()), pump(err, tx.clone())); } => {},
            _ = tx.closed() => {
                if let Some(pid) = child.id() { crate::signals::end_tree(pid); }
                let _ = child.wait().await;
                return;
            }
        }

        let status = tokio::select! {
            status = child.wait() => status,
            _ = tx.closed() => {
                if let Some(pid) = child.id() { crate::signals::end_tree(pid); }
                let _ = child.wait().await;
                return;
            }
        };
        let code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
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
    // Some tools have no login of their own — kubectl and docker take credentials from elsewhere.
    // Running `kubectl` with no arguments would print help and report success, which would look
    // like a connection that worked.
    if t.login.is_empty() {
        return refuse(&format!(
            "{} has no sign-in of its own. Its credentials come from elsewhere — see Settings.",
            t.label
        ));
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
    fn provider_checks_use_auth_state_and_matching_scope() {
        assert!(validate_connection("wrangler", r#"{"loggedIn":false}"#.into()).is_err());
        assert!(validate_connection("wrangler", "You are not authenticated".into()).is_err());
        assert!(validate_connection("wrangler", r#"{"loggedIn":true}"#.into()).is_ok());
        assert_eq!(
            condense(
                "wrangler",
                r#"{"loggedIn":true,"email":"dev@example.com","accounts":[{"name":"Example"}]}"#
            ),
            "dev@example.com · Example"
        );
        assert_eq!(tool("wrangler").unwrap().whoami, &["whoami", "--json"]);
        assert_eq!(
            tool("gh").unwrap().whoami,
            &["auth", "status", "--active", "--hostname", "github.com"]
        );
        assert_eq!(
            scoped_args(&["sts", "get-caller-identity"], Some("team-prod")),
            vec!["sts", "get-caller-identity", "--profile", "team-prod"]
        );
        assert_eq!(
            scoped_args(&["sts", "get-caller-identity"], None),
            vec!["sts", "get-caller-identity"]
        );
    }

    #[tokio::test]
    async fn disconnecting_setup_reaps_a_silent_process() {
        use axum::response::IntoResponse;
        use futures_util::StreamExt;
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo $$; exec sleep 30"]);
        let mut body = stream(command)
            .into_response()
            .into_body()
            .into_data_stream();
        let data = tokio::time::timeout(std::time::Duration::from_secs(2), body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let event = String::from_utf8(data.to_vec()).unwrap();
        let pid: u32 = event
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap()
            .parse()
            .unwrap();
        drop(body);
        for _ in 0..100 {
            if unsafe { libc::kill(pid as i32, 0) } != 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        crate::signals::end_tree(pid);
        panic!("the setup process outlived its disconnected UI");
    }

    #[test]
    fn claude_auth_requires_current_login_not_cached_account_metadata() {
        let mut status = ClaudeStatus::default();
        apply_claude_auth(
            &mut status,
            r#"{"oauthAccount":{"emailAddress":"old@example.test"}}"#,
        );
        assert!(!status.authenticated);
        apply_claude_auth(
            &mut status,
            r#"{"loggedIn":false,"email":"old@example.test"}"#,
        );
        assert!(!status.authenticated);
        assert!(status.account.is_none());
        apply_claude_auth(
            &mut status,
            r#"{"loggedIn":true,"email":"dev@example.test","subscriptionType":"max"}"#,
        );
        assert!(status.authenticated);
        assert_eq!(status.account.as_deref(), Some("dev@example.test"));
    }

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

    /// `cluster-info` prints a banner with terminal colour codes and a second line about debug
    /// dumps. What is worth showing is the control plane's address and nothing else.
    #[test]
    fn condenses_cluster_info_to_the_control_plane() {
        let raw = "Kubernetes control plane is running at https://10.0.0.1\n\
                   CoreDNS is running at https://10.0.0.1/api/v1/namespaces/kube-system";
        assert_eq!(condense("kubectl", raw), "https://10.0.0.1");
    }

    /// Homebrew is the one prerequisite Keel asks for beyond Claude Code, so every tool has to be
    /// reachable through it. A formula that only installs through bun or npm puts a JavaScript
    /// runtime back in front of somebody on a bare machine, which is the thing that line exists to
    /// remove.
    #[test]
    fn every_tool_installs_with_homebrew() {
        for t in TOOLS {
            assert!(
                t.install.iter().any(|(mgr, _)| *mgr == "brew"),
                "{} has no brew formula, so a machine with only brew cannot install it",
                t.id
            );
        }
    }

    /// Every tool is installable and documented. Not every tool is *loggable into*: `kubectl` and
    /// `docker` take their credentials from somewhere else entirely, and offering a button that
    /// runs nothing would be worse than saying where they actually come from.
    #[test]
    fn every_tool_can_be_installed_and_explained() {
        for t in TOOLS {
            assert!(!t.install.is_empty(), "{} has no installer", t.id);
            assert!(
                t.manual.starts_with("https://"),
                "{} needs a fallback link",
                t.id
            );
            assert!(!t.whoami.is_empty(), "{} cannot report its own state", t.id);
        }
    }

    /// Every tool has to be detectable by its own version command.
    ///
    /// `exists` used to assume `--version`, which `kubectl` rejects — so it reported an installed
    /// kubectl as missing, and signing in refused with "not installed yet" on a machine where it
    /// plainly was.
    #[test]
    fn a_tool_is_detected_the_way_it_asks_to_be() {
        for t in TOOLS {
            assert!(
                !t.version.is_empty(),
                "{} has no way to report its version",
                t.id
            );
        }
        let kubectl = tool("kubectl").expect("kubectl is registered");
        assert_ne!(
            kubectl.version,
            ["--version"],
            "kubectl rejects --version; detecting it that way finds nothing"
        );
    }

    /// A tool with no login flow must be one Keel explains rather than one it silently cannot
    /// connect. Adding one without a `blocked` message would leave a dead row in Settings.
    ///
    /// The frozen list of ids *was* the assertion, so adding any tool failed this test whether or
    /// not the thing it guards against had happened. It checks the property directly now: a
    /// login-less tool needs something to say and something to run.
    #[test]
    fn a_tool_without_a_login_is_one_we_explain() {
        for t in TOOLS.iter().filter(|t| t.login.is_empty()) {
            assert!(
                !t.setup.is_empty() || t.id == "kubectl",
                "{} has no login and no setup command, so Settings can offer nothing",
                t.id
            );
            assert!(
                BLOCKED_EXPLAINS.contains(&t.id),
                "{} has no login, so it needs a `blocked` message saying why",
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
