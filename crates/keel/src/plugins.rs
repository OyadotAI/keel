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
        add_source: None,
        reason: "Workers, Durable Objects and the Agents SDK — the platform this repository \
                 deploys to.",
        when: &["wrangler.jsonc", "wrangler.toml", "wrangler.json"],
    },
    Suggestion {
        name: "rust-analyzer-lsp",
        marketplace: "claude-plugins-official",
        add_source: None,
        reason: "Real symbol navigation for Rust, so the agent stops grepping for definitions.",
        when: &["Cargo.toml"],
    },
    Suggestion {
        name: "code-review",
        marketplace: "claude-plugins-official",
        add_source: None,
        reason: "An independent reviewer is the highest-ROI verification gate there is — an agent \
                 grading its own work is close to worthless.",
        when: &[],
    },
    Suggestion {
        name: "security-guidance",
        marketplace: "claude-plugins-official",
        add_source: None,
        reason: "Pattern warnings on edits. 45% of AI-generated code samples carry an OWASP Top-10 \
                 flaw, so this is not optional hygiene.",
        when: &[],
    },
    Suggestion {
        name: "github",
        marketplace: "claude-plugins-official",
        add_source: None,
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
                    entries.flatten().take(64).any(|e| e.path().join(m).exists())
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

fn valid(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
}

/// Install a plugin, adding its marketplace first when Keel knows one is needed.
pub async fn install(Query(q): Query<InstallQuery>) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    if !valid(&q.name) || !valid(&q.marketplace) {
        let t = tx.clone();
        tokio::spawn(async move {
            let _ = t
                .send(Ok(Event::default().event("fatal").data("invalid plugin id")))
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

        // Adding a marketplace that is already configured is a no-op that prints a notice, so it is
        // safe to run every time rather than checking first.
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

/// Run a command, forwarding both pipes, and return its exit code.
async fn pipe(
    command: &mut Command,
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) -> i32 {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let Ok(mut child) = command.spawn() else {
        let _ = tx
            .send(Ok(Event::default().event("line").data("could not run `claude`")))
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
    child.wait().await.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use super::*;

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
