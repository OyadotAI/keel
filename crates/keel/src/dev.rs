//! Running the project and showing it.
//!
//! Deploying and then having nowhere to look is half a loop. Keel starts the project's own dev
//! command, watches its output for the URL it prints, and hands that to a preview pane — and does
//! the same for a deploy, which prints the deployed URL.
//!
//! The process is owned by Keel rather than by the agent: a dev server is long-running, and an
//! agent tool call that never returns is a hang, not a feature.

use axum::Json;
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

#[derive(Default)]
struct DevState {
    child: Option<Child>,
    url: Option<String>,
    command: Option<String>,
    /// Recent output, capped — a dev server left running all day should not grow without bound.
    log: Vec<String>,
}

fn state() -> &'static Mutex<DevState> {
    static S: OnceLock<Mutex<DevState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(DevState::default()))
}

/// The command that runs this project locally.
///
/// The project's own `dev` script wins, because it is what the author intended `dev` to mean.
/// Falling back to `wrangler dev` only when there is a Wrangler config keeps Keel from inventing a
/// run command for a project that has none.
pub fn detect(root: &Utf8Path) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(root.join("package.json"))
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
        && json.get("scripts").and_then(|s| s.get("dev")).is_some()
    {
        let bun = root.join("bun.lock").exists() || root.join("bun.lockb").exists();
        return Some(if bun {
            "bun run dev".into()
        } else {
            "npm run dev".into()
        });
    }
    if root.join("wrangler.jsonc").exists()
        || root.join("wrangler.json").exists()
        || root.join("wrangler.toml").exists()
    {
        return Some("wrangler dev".into());
    }
    None
}

/// Pull a servable URL out of a line of tool output.
///
/// Dev servers and deploys both announce themselves this way, and the announcement is the only
/// reliable place the port appears — it is chosen at runtime when the preferred one is taken.
pub fn find_url(line: &str) -> Option<String> {
    let start = line.find("http://").or_else(|| line.find("https://"))?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ',' | '>'))
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['.', ':', ']']).to_string();

    // Ignore documentation and dashboard links that deploy output is full of.
    let noise = [
        "docs.",
        "developers.",
        "dash.",
        "github.com",
        "npmjs.com",
        "schemastore",
    ];
    if noise.iter().any(|n| url.contains(n)) {
        return None;
    }
    if url.len() <= 10 {
        return None;
    }

    // Keep only the origin. Wrangler announces internal endpoints like
    // `http://localhost:8787/cdn-cgi/local/explorer/api`, and pointing the preview at one of those
    // shows the wrong thing — the app is at the root.
    let after_scheme = url.find("://")? + 3;
    let origin_end = url[after_scheme..]
        .find('/')
        .map(|i| after_scheme + i)
        .unwrap_or(url.len());
    Some(url[..origin_end].to_string())
}

#[derive(Serialize)]
pub struct Status {
    pub running: bool,
    pub url: Option<String>,
    pub command: Option<String>,
    /// What Keel would run, when nothing is running yet.
    pub detected: Option<String>,
    pub log: Vec<String>,
}

pub async fn status(crate::serve::Checkout(repo): crate::serve::Checkout) -> Json<Status> {
    let s = state().lock().expect("dev lock");
    Json(Status {
        running: s.child.is_some(),
        url: s.url.clone(),
        command: s.command.clone(),
        detected: detect(&repo),
        log: s.log.clone(),
    })
}

#[derive(Deserialize)]
pub struct StartBody {
    /// Override the detected command.
    pub command: Option<String>,
}

pub async fn start(
    crate::serve::Checkout(repo): crate::serve::Checkout,
    Json(body): Json<StartBody>,
) -> Result<Json<Status>, (axum::http::StatusCode, String)> {
    let bad = |m: String| (axum::http::StatusCode::BAD_REQUEST, m);

    {
        let s = state().lock().expect("dev lock");
        if s.child.is_some() {
            return Err(bad("A dev server is already running. Stop it first.".into()));
        }
    }

    let command = body
        .command
        .filter(|c| !c.trim().is_empty())
        .or_else(|| detect(&repo))
        .ok_or_else(|| bad("No dev command found. Add a `dev` script to package.json.".into()))?;

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(&repo)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| bad(e.to_string()))?;

    let (out, err) = (child.stdout.take(), child.stderr.take());
    {
        let mut s = state().lock().expect("dev lock");
        s.child = Some(child);
        s.command = Some(command.clone());
        s.url = None;
        s.log.clear();
    }

    tokio::spawn(watch(out));
    tokio::spawn(watch(err));

    Ok(Json(Status {
        running: true,
        url: None,
        command: Some(command),
        detected: detect(&repo),
        log: Vec::new(),
    }))
}

/// Keep the log and pick up the URL when the server announces it.
async fn watch<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) {
    const MAX_LOG: usize = 400;
    let Some(pipe) = pipe else { return };
    let mut lines = BufReader::new(pipe).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let mut s = state().lock().expect("dev lock");
        if s.url.is_none()
            && let Some(url) = find_url(&line)
        {
            s.url = Some(url);
        }
        s.log.push(line);
        if s.log.len() > MAX_LOG {
            let excess = s.log.len() - MAX_LOG;
            s.log.drain(..excess);
        }
    }
}

pub async fn stop() -> Json<bool> {
    let mut s = state().lock().expect("dev lock");
    if let Some(child) = s.child.as_mut() {
        let _ = child.start_kill();
    }
    s.child = None;
    s.url = None;
    Json(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use tempfile::TempDir;

    fn repo(files: &[(&str, &str)]) -> (TempDir, Utf8PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        for (p, b) in files {
            std::fs::write(root.join(p), b).expect("write");
        }
        (dir, root)
    }

    #[test]
    fn a_dev_script_beats_the_wrangler_default() {
        let (_d, root) = repo(&[
            ("package.json", r#"{"scripts":{"dev":"vite"}}"#),
            ("wrangler.jsonc", "{}"),
            ("bun.lock", ""),
        ]);
        assert_eq!(detect(&root).unwrap(), "bun run dev");
    }

    #[test]
    fn wrangler_is_the_fallback_only_with_a_config() {
        let (_d, root) = repo(&[("wrangler.toml", "")]);
        assert_eq!(detect(&root).unwrap(), "wrangler dev");
        let (_d2, bare) = repo(&[("README.md", "")]);
        assert!(detect(&bare).is_none());
    }

    #[test]
    fn finds_the_url_a_dev_server_announces() {
        assert_eq!(
            find_url("  Ready on http://localhost:8787").as_deref(),
            Some("http://localhost:8787")
        );
        assert_eq!(
            find_url("Deployed to https://demo.workers.dev").as_deref(),
            Some("https://demo.workers.dev")
        );
        // Trailing punctuation is not part of the URL.
        assert_eq!(
            find_url("see http://127.0.0.1:3000.").as_deref(),
            Some("http://127.0.0.1:3000")
        );
        // Wrangler announces internal endpoints; the app is at the origin, not down that path.
        assert_eq!(
            find_url("Ready http://localhost:8787/cdn-cgi/local/explorer/api").as_deref(),
            Some("http://localhost:8787")
        );
    }

    #[test]
    fn documentation_links_are_not_mistaken_for_the_app() {
        assert!(find_url("Read more at https://developers.cloudflare.com/workers").is_none());
        assert!(find_url("Open the dashboard: https://dash.cloudflare.com").is_none());
        assert!(find_url("no url here").is_none());
    }
}
