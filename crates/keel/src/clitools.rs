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
    command.arg("-lc").arg(COMMAND);
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
        brew: exists("brew"),
        brew_install: BREW_INSTALL,
        ..Default::default()
    };

    if let Ok(v) = std::process::Command::new("claude")
        .arg("--version")
        .output()
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
        out.authenticated = account
            .get("emailAddress")
            .and_then(|v| v.as_str())
            .is_some();
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
        setup: "",
    },
    Tool {
        id: "wrangler",
        label: "Wrangler",
        binary: "wrangler",
        version: &["--version"],
        whoami: &["whoami"],
        // brew first, and its formula declares node as a dependency, so a bare machine gets both
        // from one command. bun and npm stay as fallbacks for anyone who has a runtime but not
        // Homebrew.
        install: &[
            ("brew", &["install", "cloudflare-wrangler"]),
            ("bun", &["add", "--global", "wrangler"]),
            ("npm", &["install", "--global", "wrangler"]),
        ],
        login: &["login"],
        identity: &["whoami"],
        manual: "https://developers.cloudflare.com/workers/wrangler/install-and-update/",
        setup: "",
    },
    Tool {
        id: "gcloud",
        label: "Google Cloud",
        binary: "gcloud",
        version: &["--version"],
        whoami: &[
            "auth",
            "list",
            "--filter=status:ACTIVE",
            "--format=value(account)",
        ],
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
        // Credentials come from the provider. Listing what is already configured is the honest
        // first step, and it is safe to run unprompted — unlike a half-typed `get-credentials`.
        setup: "kubectl config get-contexts",
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
];

/// Profiles the AWS CLI can see, across both `~/.aws/config` and `~/.aws/credentials`.
///
/// `aws configure list-profiles` rather than parsing the INI: it sees SSO profiles in `config` and
/// key-based ones in `credentials`, and it is the CLI's own answer to the question rather than
/// Keel's guess at it.
fn aws_profiles() -> Vec<String> {
    std::process::Command::new("aws")
        .args(["configure", "list-profiles"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
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
pub async fn status() -> axum::Json<Vec<ToolStatus>> {
    let checks = TOOLS.iter().map(|t| async move {
        // Within one tool the calls are ordered — asking a missing binary who it is wastes a
        // process spawn, and asking an unauthenticated one wastes a network round trip.
        let version = run(t.binary, t.version)
            .await
            .and_then(|o| o.lines().next().map(|l| l.trim().to_owned()));

        let authenticated = version.is_some() && run(t.binary, t.whoami).await.is_some();

        let identity = if authenticated {
            run(t.binary, t.identity)
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
            aws_profiles()
        } else {
            Vec::new()
        };

        // Why it is not connected, in its own words. A tool with no login of its own takes its
        // credentials from somewhere else, and naming that is more use than a button that runs
        // nothing — but better still is a command, which the UI can hand to the terminal.
        let blocked = match t.id {
            "kubectl" if version.is_some() && !authenticated => Some(
                "No cluster is reachable. Pick one below, or point kubectl at it yourself."
                    .to_string(),
            ),
            "docker" if version.is_some() && !authenticated => {
                Some("Docker is installed but its daemon is not running.".to_string())
            }
            "aws" if version.is_some() && profiles.is_empty() => Some(
                "No AWS profile exists yet. Set one up with Identity Center below, or run \
                 `aws configure` for an access key — Keel never asks for one."
                    .to_string(),
            ),
            "aws" if version.is_some() && !authenticated => Some(
                "A profile exists but its credentials are not valid. If it uses Identity Center, \
                 signing in again will refresh it."
                    .to_string(),
            ),
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
            manual: t.manual,
        }
    });

    axum::Json(futures_util::future::join_all(checks).await)
}

/// Run a command and give back its stdout, or nothing if it failed.
async fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().await.ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
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
    /// connect. Adding a fourth without a `blocked` message would leave a dead row in Settings.
    #[test]
    fn a_tool_without_a_login_is_one_we_explain() {
        let no_login: Vec<&str> = TOOLS
            .iter()
            .filter(|t| t.login.is_empty())
            .map(|t| t.id)
            .collect();
        assert_eq!(no_login, vec!["kubectl", "docker"]);
    }

    #[test]
    fn tool_ids_are_unique_and_resolvable() {
        for t in TOOLS {
            assert_eq!(tool(t.id).map(|x| x.id), Some(t.id));
        }
        assert!(tool("definitely-not-a-tool").is_none());
    }
}
