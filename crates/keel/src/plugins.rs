//! The plugin catalog, and installing from it.
//!
//! Claude Code already has a marketplace, a catalog cache and an install command. Keel does not
//! reimplement any of that — it reads the same cache and shells out to `claude plugin`, so what you
//! install here is installed everywhere, and uninstalling from the CLI is reflected here.

use axum::extract::Query;
use axum::response::sse::{Event, Sse};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::convert::Infallible;
use std::process::Stdio;
use tokio::process::Command;
use tokio_stream::wrappers::ReceiverStream;

#[derive(Debug, Clone, Serialize)]
pub struct Plugin {
    pub name: String,
    pub description: String,
    pub author: Option<String>,
    pub category: Option<String>,
    pub homepage: Option<String>,
    /// Marketplace it belongs to, for the `name@marketplace` install id.
    pub marketplace: String,
    pub installed: bool,
    /// Why Keel is suggesting this one for the open repository.
    pub reason: Option<String>,
}

/// A plugin worth suggesting, and the repository signal that justifies it.
struct Suggestion {
    name: &'static str,
    marketplace: &'static str,
    /// Marketplace to add first, when it is not one of the defaults.
    add_source: Option<&'static str>,
    reason: &'static str,
    /// File markers in the repo that make this relevant. Empty means always relevant.
    when: &'static [&'static str],
}

/// The curated set.
///
/// Deliberately short. A recommendations list that suggests thirty things is a directory, and a
/// directory is what the full catalog already is — the value here is having an opinion.
const SUGGESTED: &[Suggestion] = &[
    Suggestion {
        name: "ponytail",
        marketplace: "ponytail",
        add_source: Some("DietrichGebert/ponytail"),
        reason: "Pushes the agent toward the smallest change that works, before it reaches for a \
                 dependency or a wrapper. Directly targets the duplication-up, refactoring-down \
                 pattern this scanner exists to catch.",
        when: &[],
    },
    Suggestion {
        name: "cloudflare",
        marketplace: "claude-plugins-official",
        add_source: Some("anthropics/claude-plugins-official"),
        reason: "Workers, Durable Objects and the Agents SDK — the platform this repository \
                 deploys to.",
        when: &["wrangler.jsonc", "wrangler.toml", "wrangler.json"],
    },
    Suggestion {
        name: "rust-analyzer-lsp",
        marketplace: "claude-plugins-official",
        add_source: Some("anthropics/claude-plugins-official"),
        reason: "Real symbol navigation for Rust, so the agent stops grepping for definitions.",
        when: &["Cargo.toml"],
    },
    Suggestion {
        name: "code-review",
        marketplace: "claude-plugins-official",
        add_source: Some("anthropics/claude-plugins-official"),
        reason: "An independent reviewer is the highest-ROI verification gate there is — an agent \
                 grading its own work is close to worthless.",
        when: &[],
    },
    Suggestion {
        name: "security-guidance",
        marketplace: "claude-plugins-official",
        add_source: Some("anthropics/claude-plugins-official"),
        reason: "Pattern warnings on edits. 45% of AI-generated code samples carry an OWASP Top-10 \
                 flaw, so this is not optional hygiene.",
        when: &[],
    },
    Suggestion {
        name: "github",
        marketplace: "claude-plugins-official",
        add_source: Some("anthropics/claude-plugins-official"),
        reason: "Issues and pull requests without leaving the session.",
        when: &[".github"],
    },
];

fn claude_home() -> Option<Utf8PathBuf> {
    keel_workspace::claude_home()
}

/// Every plugin in the cached marketplace catalog.
fn catalog() -> Vec<Plugin> {
    let Some(home) = claude_home() else {
        return Vec::new();
    };
    let path = home.join("plugins").join("plugin-catalog-cache.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(root) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };

    // The cache nests plugins under marketplace keys whose shape is not contractual, so walk for
    // anything that looks like an entry rather than assuming a path.
    let mut out = Vec::new();
    collect(root.get("catalog").unwrap_or(&Value::Null), "", &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

fn collect(node: &Value, marketplace: &str, out: &mut Vec<Plugin>) {
    match node {
        Value::Object(map) => {
            let has_entry = map.contains_key("name") && map.contains_key("description");
            if has_entry {
                let text = |k: &str| map.get(k).and_then(Value::as_str).map(str::to_owned);
                out.push(Plugin {
                    name: text("name").unwrap_or_default(),
                    description: text("description").unwrap_or_default(),
                    author: text("author"),
                    category: text("category"),
                    homepage: text("homepage"),
                    marketplace: if marketplace.is_empty() {
                        "claude-plugins-official".to_string()
                    } else {
                        marketplace.to_string()
                    },
                    installed: false,
                    reason: None,
                });
                return;
            }
            for (key, value) in map {
                // A key holding a list of entries is almost always the marketplace name.
                let next = if value.is_array() || value.is_object() {
                    key.as_str()
                } else {
                    marketplace
                };
                collect(value, next, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect(i, marketplace, out)),
        _ => {}
    }
}

/// Names of plugins already installed, from Claude Code's own index.
fn installed() -> Vec<String> {
    let Some(home) = claude_home() else {
        return Vec::new();
    };
    let path = home.join("plugins").join("installed_plugins.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(root) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    root.get("plugins")
        .and_then(Value::as_object)
        .map(|m| {
            m.keys()
                .map(|k| k.split('@').next().unwrap_or(k).to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Serialize)]
pub struct CatalogResponse {
    pub suggested: Vec<Plugin>,
    pub all: Vec<Plugin>,
}

#[derive(Deserialize)]
pub struct CatalogQuery {
    /// Repository root, so suggestions can depend on what is actually in it.
    pub repo: Option<String>,
}

pub async fn list(Query(q): Query<CatalogQuery>) -> axum::Json<CatalogResponse> {
    let have = installed();
    let mut all = catalog();
    for p in &mut all {
        p.installed = have.contains(&p.name);
    }

    let repo = q.repo.map(Utf8PathBuf::from);
    let relevant = |markers: &[&str]| -> bool {
        if markers.is_empty() {
            return true;
        }
        let Some(root) = &repo else { return true };
        // Shallow check: a marker at the root, or one directory down for monorepos.
        markers.iter().any(|m| {
            root.join(m).exists()
                || std::fs::read_dir(root).is_ok_and(|entries| {
                    entries
                        .flatten()
                        .take(64)
                        .any(|e| e.path().join(m).exists())
                })
        })
    };

    let suggested = SUGGESTED
        .iter()
        .filter(|s| relevant(s.when))
        .map(|s| {
            let known = all.iter().find(|p| p.name == s.name);
            Plugin {
                name: s.name.to_string(),
                description: known.map(|p| p.description.clone()).unwrap_or_default(),
                author: known.and_then(|p| p.author.clone()),
                category: known.and_then(|p| p.category.clone()),
                homepage: known.and_then(|p| p.homepage.clone()),
                marketplace: s.marketplace.to_string(),
                installed: have.contains(&s.name.to_string()),
                reason: Some(s.reason.to_string()),
            }
        })
        .collect();

    axum::Json(CatalogResponse { suggested, all })
}

#[derive(Deserialize)]
pub struct InstallQuery {
    pub name: String,
    pub marketplace: String,
}

/// What a plugin actually contains, from `claude plugin details`.
///
/// This is how you find a *skill* rather than a plugin: the catalog lists packages, and a package's
/// value is the skills inside it. The token cost matters too — an always-on cost is paid by every
/// session whether the skill fires or not.
#[derive(Serialize, Default)]
pub struct Details {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub skills: Vec<String>,
    pub agents: Vec<String>,
    pub hooks: Vec<String>,
    pub mcp: Vec<String>,
    /// Tokens added to every session, as printed.
    pub always_on: Option<String>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
pub struct DetailsQuery {
    pub name: String,
}

/// Read a plugin's component inventory.
pub async fn details(Query(q): Query<DetailsQuery>) -> axum::Json<Details> {
    if !valid(&q.name) {
        return axum::Json(Details {
            error: Some("invalid plugin name".into()),
            ..Default::default()
        });
    }

    let out = std::process::Command::new("claude")
        .args(["plugin", "details", &q.name])
        .output();
    let Ok(out) = out else {
        return axum::Json(Details {
            name: q.name,
            error: Some("could not run `claude`".into()),
            ..Default::default()
        });
    };
    if !out.status.success() {
        return axum::Json(Details {
            name: q.name,
            error: Some(
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .next()
                    .unwrap_or("not installed")
                    .to_string(),
            ),
            ..Default::default()
        });
    }

    axum::Json(parse_details(
        &q.name,
        &String::from_utf8_lossy(&out.stdout),
    ))
}

/// Parse the human-readable inventory.
///
/// Shelling out and parsing beats reading the plugin directory: the CLI already resolves which
/// components a plugin contributes after its manifest and any conditional loading, and that
/// resolution is not something to reimplement.
fn parse_details(name: &str, text: &str) -> Details {
    let mut d = Details {
        name: name.to_string(),
        ..Default::default()
    };

    // `Skills (6)  a, b, c` — the count is in parentheses, the names follow.
    let list = |line: &str, label: &str| -> Option<Vec<String>> {
        let rest = line.trim().strip_prefix(label)?;
        let rest = rest.split_once(')')?.1;
        Some(
            rest.split(',')
                .map(|s| s.split("  (").next().unwrap_or(s).trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        )
    };

    for line in text.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix(name).map(str::trim)
            && d.version.is_none()
            && !v.is_empty()
            && v.chars().next().is_some_and(|c| c.is_ascii_digit())
        {
            d.version = Some(v.to_string());
        }
        if let Some(v) = t.strip_prefix("Description:") {
            d.description = Some(v.trim().to_string());
        }
        if let Some(v) = list(t, "Skills (") {
            d.skills = v;
        }
        if let Some(v) = list(t, "Agents (") {
            d.agents = v;
        }
        if let Some(v) = list(t, "Hooks (") {
            d.hooks = v;
        }
        if let Some(v) = list(t, "MCP servers (") {
            d.mcp = v;
        }
        if let Some(v) = t.strip_prefix("Always-on:") {
            d.always_on = Some(
                v.split("added")
                    .next()
                    .unwrap_or(v)
                    .trim()
                    .trim_start_matches('~')
                    .to_string(),
            );
        }
    }
    d
}

#[derive(Deserialize)]
pub struct ActionQuery {
    /// One of `uninstall`, `enable`, `disable`, `update`.
    pub action: String,
    pub name: String,
    pub marketplace: Option<String>,
}

/// A configured marketplace.
#[derive(Serialize)]
pub struct Marketplace {
    pub name: String,
    pub source: String,
}

/// Marketplaces Claude Code has configured, parsed from `claude plugin marketplace list`.
///
/// Shelling out rather than reading a file: the on-disk layout is not a contract, and the CLI is.
pub async fn marketplaces() -> axum::Json<Vec<Marketplace>> {
    let out = std::process::Command::new("claude")
        .args(["plugin", "marketplace", "list"])
        .output();
    let Ok(out) = out else {
        return axum::Json(Vec::new());
    };

    let text = String::from_utf8_lossy(&out.stdout);
    let mut list = Vec::new();
    let mut name: Option<String> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('❯') {
            name = Some(rest.trim().to_string());
        } else if let Some(src) = t.strip_prefix("Source:")
            && let Some(n) = name.take()
        {
            list.push(Marketplace {
                name: n,
                source: src.trim().to_string(),
            });
        }
    }
    axum::Json(list)
}

/// Run one plugin lifecycle action.
///
/// The action set is closed rather than passed through, so a crafted request cannot reach an
/// arbitrary `claude plugin` subcommand.
pub async fn action(
    Query(q): Query<ActionQuery>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let verb = match q.action.as_str() {
        "uninstall" => "uninstall",
        "enable" => "enable",
        "disable" => "disable",
        "update" => "update",
        _ => return refuse("unknown action"),
    };
    if !valid(&q.name) {
        return refuse("invalid plugin name");
    }

    // enable/disable take a bare name; install/uninstall/update accept name@marketplace.
    let target = match (&q.marketplace, verb) {
        (Some(m), "uninstall" | "update") if valid(m) => format!("{}@{m}", q.name),
        _ => q.name.clone(),
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(async move {
        let _ = tx
            .send(Ok(Event::default()
                .event("line")
                .data(format!("$ claude plugin {verb} {target}"))))
            .await;
        let code = pipe(Command::new("claude").args(["plugin", verb, &target]), &tx).await;
        let _ = tx
            .send(Ok(Event::default().event("done").data(code.to_string())))
            .await;
    });
    Sse::new(ReceiverStream::new(rx))
}

/// Refresh every marketplace, which is how updates become visible.
pub async fn refresh_marketplaces() -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(async move {
        let _ = tx
            .send(Ok(Event::default()
                .event("line")
                .data("$ claude plugin marketplace update")))
            .await;
        let code = pipe(
            Command::new("claude").args(["plugin", "marketplace", "update"]),
            &tx,
        )
        .await;
        let _ = tx
            .send(Ok(Event::default().event("done").data(code.to_string())))
            .await;
    });
    Sse::new(ReceiverStream::new(rx))
}

fn valid(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
}

/// Install a plugin, adding its marketplace first when Keel knows one is needed.
pub async fn install(
    Query(q): Query<InstallQuery>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    if !valid(&q.name) || !valid(&q.marketplace) {
        let t = tx.clone();
        tokio::spawn(async move {
            let _ = t
                .send(Ok(Event::default()
                    .event("fatal")
                    .data("invalid plugin id")))
                .await;
        });
        return Sse::new(ReceiverStream::new(rx));
    }

    let add_source = SUGGESTED
        .iter()
        .find(|s| s.name == q.name)
        .and_then(|s| s.add_source);

    tokio::spawn(async move {
        let send = |line: String| {
            let tx = tx.clone();
            async move {
                let _ = tx.send(Ok(Event::default().event("line").data(line))).await;
            }
        };

        // Every suggestion names the marketplace it comes from, including the official one.
        // Assuming that one was already configured worked on a machine where it had been added at
        // some point and failed on a fresh one, with `claude` reporting a marketplace it had never
        // heard of — an error nobody could act on from inside Keel.
        //
        // Adding one that is already configured is a no-op that prints a notice, so it is safe to
        // run every time rather than checking first.
        if let Some(source) = add_source {
            send(format!("$ claude plugin marketplace add {source}")).await;
            let _ = pipe(
                Command::new("claude").args(["plugin", "marketplace", "add", source]),
                &tx,
            )
            .await;
        }

        let id = format!("{}@{}", q.name, q.marketplace);
        send(format!("$ claude plugin install {id}")).await;
        let code = pipe(Command::new("claude").args(["plugin", "install", &id]), &tx).await;

        let _ = tx
            .send(Ok(Event::default().event("done").data(code.to_string())))
            .await;
    });

    Sse::new(ReceiverStream::new(rx))
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

/// Run a command, forwarding both pipes, and return its exit code.
pub(crate) async fn pipe(
    command: &mut Command,
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) -> i32 {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let Ok(mut child) = command.spawn() else {
        let _ = tx
            .send(Ok(Event::default()
                .event("line")
                .data("could not run `claude`")))
            .await;
        return -1;
    };

    let (out, err) = (child.stdout.take(), child.stderr.take());
    // Shared with clitools: generic over the pipe type, because a closure monomorphises to
    // whichever of stdout/stderr is passed first.
    tokio::join!(
        crate::clitools::pump(out, tx.clone()),
        crate::clitools::pump(err, tx.clone())
    );
    child
        .wait()
        .await
        .map(|s| s.code().unwrap_or(-1))
        .unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_component_inventory() {
        let text = "ponytail 4.9.0\n              Description: Lazy senior dev mode.\n              Source: ponytail@ponytail\n\n            Component inventory\n              Skills (6)  ponytail, ponytail-audit, ponytail-help\n              Agents (0)\n              Hooks (3)  SessionStart, SubagentStart  (harness-only — no model context cost)\n              MCP servers (0)\n\n            Projected token cost\n              Always-on:   ~983 tok   added to every session\n";
        let d = parse_details("ponytail", text);
        assert_eq!(d.version.as_deref(), Some("4.9.0"));
        assert_eq!(d.skills.len(), 3);
        assert_eq!(d.skills[0], "ponytail");
        assert!(d.agents.is_empty());
        // The trailing annotation must not become part of a hook name.
        assert_eq!(d.hooks, vec!["SessionStart", "SubagentStart"]);
        assert_eq!(d.always_on.as_deref(), Some("983 tok"));
    }

    #[test]
    fn install_ids_are_validated() {
        assert!(valid("ponytail"));
        assert!(valid("claude-plugins-official"));
        assert!(!valid("a; rm -rf /"));
        assert!(!valid(""));
        assert!(!valid("x$(whoami)"));
    }

    #[test]
    fn every_suggestion_carries_a_reason() {
        for s in SUGGESTED {
            assert!(s.reason.len() > 20, "{} needs a real reason", s.name);
            assert!(valid(s.name) && valid(s.marketplace));
        }
    }
}
