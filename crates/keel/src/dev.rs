//! Running the project and showing it.
//!
//! Deploying and then having nowhere to look is half a loop. Keel starts the project's own dev
//! command, watches its output for the URL it prints, and hands that to a preview pane — and does
//! the same for a deploy, which prints the deployed URL.
//!
//! The process is owned by Keel rather than by the agent: a dev server is long-running, and an
//! agent tool call that never returns is a hang, not a feature.

use crate::lock::Locked;
use axum::Json;
use camino::{Utf8Path, Utf8PathBuf};
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
    /// The checkout it was started in.
    ///
    /// One dev server for the whole daemon is a decision, not an accident — ports are not
    /// allocated per lane. What was an accident is that nothing recorded *whose* it was, so
    /// `status` answered "running", with that URL, to every lane. A lane with its own checkout
    /// opened the preview, saw green, and reviewed another lane's rendering of another lane's
    /// worktree against its own diff — and the design turn's pixel check then re-photographed an
    /// element served from the wrong tree and returned a verdict about it.
    checkout: Option<Utf8PathBuf>,
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
/// A dev server Keel can start, and where to start it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Dev {
    pub command: String,
    /// Relative to the repository, empty when it is the repository itself. A monorepo runs its
    /// web app from a subdirectory, and running `npm run dev` at the root does nothing there.
    pub dir: String,
}

/// The dev server for this repository, looked for where people actually put one.
///
/// The root first, then one level down, then under `apps/` and `packages/` — the three shapes a
/// JavaScript monorepo comes in. It used to look only at the root, so a repository whose web app
/// lives in `dashboard/` reported "This project has no dev command" while sitting on a
/// `package.json` with `"dev": "next dev"` one directory away.
///
/// When more than one candidate has a `dev` script, the one that renders pages wins: the preview
/// is a browser, so a Next or Vite app is what somebody wants to see, not the API beside it.
pub fn detect(root: &Utf8Path) -> Option<Dev> {
    let mut candidates: Vec<Dev> = Vec::new();
    if let Some(d) = detect_in(root, "") {
        // The root is not a guess. If it declares one, that is the answer.
        return Some(d);
    }
    for dir in searchable(root) {
        if let Some(d) = detect_in(&root.join(&dir), &dir) {
            candidates.push(d);
        }
    }
    candidates
        .iter()
        .find(|d| renders_pages(&root.join(&d.dir)))
        .or_else(|| candidates.first())
        .cloned()
}

/// Directories worth looking in: the immediate children, and the children of `apps` / `packages`.
fn searchable(root: &Utf8Path) -> Vec<String> {
    fn children(dir: &Utf8Path, prefix: &str, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.starts_with('.') && n != "node_modules" && n != "target")
            .collect();
        names.sort();
        for n in names {
            out.push(if prefix.is_empty() {
                n
            } else {
                format!("{prefix}/{n}")
            });
        }
    }

    let mut out = Vec::new();
    children(root, "", &mut out);
    for nest in ["apps", "packages"] {
        let dir = root.join(nest);
        if dir.is_dir() {
            children(&dir, nest, &mut out);
        }
    }
    out.truncate(60);
    out
}

fn detect_in(dir: &Utf8Path, rel: &str) -> Option<Dev> {
    let here = |command: &str| {
        Some(Dev {
            command: command.into(),
            dir: rel.to_string(),
        })
    };
    if let Ok(text) = std::fs::read_to_string(dir.join("package.json"))
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
        && json.get("scripts").and_then(|s| s.get("dev")).is_some()
    {
        // The lockfile is the repository's, not the package's: a workspace has one at the root.
        let bun = dir.join("bun.lock").exists() || dir.join("bun.lockb").exists();
        return here(if bun { "bun run dev" } else { "npm run dev" });
    }
    if dir.join("wrangler.jsonc").exists()
        || dir.join("wrangler.json").exists()
        || dir.join("wrangler.toml").exists()
    {
        return here("wrangler dev");
    }
    None
}

/// Whether this package is the one that draws pages, rather than the API beside it.
fn renders_pages(dir: &Utf8Path) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else {
        return false;
    };
    [
        "next",
        "vite",
        "astro",
        "react-scripts",
        "@sveltejs/kit",
        "nuxt",
        "@remix-run",
    ]
    .iter()
    .any(|framework| text.contains(&format!("\"{framework}\"")))
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
    /// True when the running server belongs to some other checkout than the one that asked.
    /// The preview says so rather than passing it off as this lane's.
    #[serde(default)]
    pub elsewhere: bool,
    /// The lane it is running in, or empty for the project itself. Only set when `elsewhere`.
    #[serde(default)]
    pub owner: String,
    /// What Keel would run, when nothing is running yet, and where.
    pub detected: Option<String>,
    /// The subdirectory the detected command runs in, empty at the repository root.
    #[serde(default)]
    pub detected_dir: String,
    pub log: Vec<String>,
}

/// The lane a checkout belongs to, for saying whose dev server this is.
fn owner_of(checkout: &Utf8Path) -> String {
    checkout
        .as_str()
        .rsplit_once(&format!("{}/", crate::worktree::DIR))
        .map(|(_, name)| name.to_string())
        .unwrap_or_default()
}

pub async fn status(crate::serve::Checkout(repo): crate::serve::Checkout) -> Json<Status> {
    let found = detect(&repo);
    let s = state().locked();
    let running = s.child.is_some();
    // Whose it is decides what this lane is told. Answering "running" with a URL served from
    // somewhere else is the one thing this must not do.
    let elsewhere = running && s.checkout.as_deref().is_some_and(|c| c != repo);
    Json(Status {
        running,
        url: if elsewhere { None } else { s.url.clone() },
        command: s.command.clone(),
        elsewhere,
        owner: if elsewhere {
            s.checkout.as_deref().map(owner_of).unwrap_or_default()
        } else {
            String::new()
        },
        detected: found.as_ref().map(|d| d.command.clone()),
        detected_dir: found.as_ref().map(|d| d.dir.clone()).unwrap_or_default(),
        log: if elsewhere { Vec::new() } else { s.log.clone() },
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
        let s = state().locked();
        if s.child.is_some() {
            let whose = match s.checkout.as_deref() {
                Some(c) if c != repo => {
                    let owner = owner_of(c);
                    if owner.is_empty() {
                        " for the project itself".to_string()
                    } else {
                        format!(" for “{owner}”")
                    }
                }
                _ => String::new(),
            };
            return Err(bad(format!(
                "A dev server is already running{whose}. Keel runs one at a time — stop it first."
            )));
        }
    }

    // A typed command runs where the person is; a detected one runs where it was found, which
    // for a monorepo is not the repository root.
    let (command, dir) = match body.command.filter(|c| !c.trim().is_empty()) {
        Some(typed) => (typed, repo.clone()),
        None => {
            let found = detect(&repo).ok_or_else(|| {
                bad("No dev command found. Add a `dev` script to package.json.".into())
            })?;
            let dir = if found.dir.is_empty() {
                repo.clone()
            } else {
                repo.join(&found.dir)
            };
            (found.command, dir)
        }
    };

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // Its own group. A dev server is never one process: `sh -c "pnpm dev"` becomes pnpm,
        // which starts the framework, which starts workers — and it is the workers that hold the
        // port. Without a group there is nothing to signal but the top of that, and Stop then
        // reports success over a server that is still listening.
        .process_group(0)
        .spawn()
        .map_err(|e| bad(e.to_string()))?;

    let (out, err) = (child.stdout.take(), child.stderr.take());
    {
        let mut s = state().locked();
        s.child = Some(child);
        s.command = Some(command.clone());
        s.checkout = Some(repo.clone());
        s.url = None;
        s.log.clear();
    }

    tokio::spawn(watch(out));
    tokio::spawn(watch(err));

    let found = detect(&repo);
    Ok(Json(Status {
        running: true,
        url: None,
        command: Some(command),
        elsewhere: false,
        owner: String::new(),
        detected: found.as_ref().map(|d| d.command.clone()),
        detected_dir: found.map(|d| d.dir).unwrap_or_default(),
        log: Vec::new(),
    }))
}

/// Keep the log and pick up the URL when the server announces it.
async fn watch<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) {
    const MAX_LOG: usize = 400;
    let Some(pipe) = pipe else { return };
    let mut lines = BufReader::new(pipe).lines();

    loop {
        let line = match crate::lines::next(&mut lines).await {
            crate::lines::Next::Line(line) => line,
            crate::lines::Next::Skipped => continue,
            crate::lines::Next::Done => break,
        };
        let announced = {
            let mut s = state().locked();
            let mut announced = false;
            if s.url.is_none()
                && let Some(url) = find_url(&line)
            {
                s.url = Some(url);
                announced = true;
            }
            s.log.push(line);
            if s.log.len() > MAX_LOG {
                let excess = s.log.len() - MAX_LOG;
                s.log.drain(..excess);
            }
            announced
        };
        if announced {
            crate::events::emit("dev.changed", None, serde_json::Value::Null);
        }
    }
    // The pipe closed: the server is gone, or going.
    crate::events::emit("dev.changed", None, serde_json::Value::Null);
}

pub async fn stop() -> Json<bool> {
    stop_now();
    Json(true)
}

/// Stop the dev server, from anywhere — including the parent-death path, which is not async.
///
/// `watch_parent` already stopped the monitored jobs on the way out, with a comment saying "a dev
/// server nobody can see and nobody can stop is worse than one that never started". The dev server
/// this module owns is not a monitored job, so it was the one thing that comment names and did not
/// cover: quitting Keel left it running, holding its port, with nothing on the machine that knew
/// what it was.
pub fn stop_now() {
    let mut s = state().locked();
    if let Some(child) = s.child.as_mut() {
        // The group, so the framework's workers go too — they are what holds the port.
        crate::signals::end_tree(child.id().unwrap_or(0));
        let _ = child.start_kill();
    }
    s.child = None;
    s.url = None;
    s.checkout = None;
    drop(s);
    crate::events::emit("dev.changed", None, serde_json::Value::Null);
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
        let found = detect(&root).unwrap();
        assert_eq!(found.command, "bun run dev");
        assert_eq!(found.dir, "", "the root is not a subdirectory");
    }

    #[test]
    fn wrangler_is_the_fallback_only_with_a_config() {
        let (_d, root) = repo(&[("wrangler.toml", "")]);
        assert_eq!(detect(&root).unwrap().command, "wrangler dev");
        let (_d2, bare) = repo(&[("README.md", "")]);
        assert!(detect(&bare).is_none());
    }

    /// Build a repository with files in subdirectories.
    fn nested(files: &[(&str, &str)]) -> (TempDir, Utf8PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        for (p, b) in files {
            let full = root.join(p);
            std::fs::create_dir_all(full.parent().unwrap()).expect("mkdir");
            std::fs::write(full, b).expect("write");
        }
        (dir, root)
    }

    /// The shape that reported "This project has no dev command" while sitting on a `next dev`
    /// one directory away: no root `package.json` at all, two packages, one of them a web app.
    #[test]
    fn a_monorepo_web_app_is_found_one_level_down() {
        let (_d, root) = nested(&[
            (
                "dashboard/package.json",
                r#"{"scripts":{"dev":"next dev"},"dependencies":{"next":"15"}}"#,
            ),
            ("api/package.json", r#"{"scripts":{"start":"node ."}}"#),
        ]);
        let found = detect(&root).unwrap();
        assert_eq!(found.command, "npm run dev");
        assert_eq!(
            found.dir, "dashboard",
            "the command has to run where it was found"
        );
    }

    /// The preview is a browser, so when two packages could both be started the one that draws
    /// pages is the one somebody wants to see.
    #[test]
    fn the_package_that_renders_pages_wins() {
        let (_d, root) = nested(&[
            ("api/package.json", r#"{"scripts":{"dev":"tsx watch src"}}"#),
            (
                "web/package.json",
                r#"{"scripts":{"dev":"vite"},"devDependencies":{"vite":"5"}}"#,
            ),
        ]);
        // `api` sorts first, so without the preference it would win on alphabetical order alone.
        assert_eq!(detect(&root).unwrap().dir, "web");
    }

    /// The other two shapes a JavaScript monorepo comes in.
    #[test]
    fn apps_and_packages_are_searched_too() {
        let (_d, apps) = nested(&[(
            "apps/site/package.json",
            r#"{"scripts":{"dev":"next dev"},"dependencies":{"next":"15"}}"#,
        )]);
        assert_eq!(detect(&apps).unwrap().dir, "apps/site");

        let (_d2, packages) = nested(&[(
            "packages/ui/package.json",
            r#"{"scripts":{"dev":"vite"},"devDependencies":{"vite":"5"}}"#,
        )]);
        assert_eq!(detect(&packages).unwrap().dir, "packages/ui");
    }

    /// A root that declares its own is not a guess, and is not overruled by a child.
    #[test]
    fn the_root_wins_when_it_has_one() {
        let (_d, root) = nested(&[
            ("package.json", r#"{"scripts":{"dev":"turbo dev"}}"#),
            (
                "web/package.json",
                r#"{"scripts":{"dev":"vite"},"devDependencies":{"vite":"5"}}"#,
            ),
        ]);
        assert_eq!(detect(&root).unwrap().dir, "");
    }

    /// Dependencies are not packages of this repository.
    #[test]
    fn node_modules_is_not_searched() {
        let (_d, root) = nested(&[(
            "node_modules/some-pkg/package.json",
            r#"{"scripts":{"dev":"vite"}}"#,
        )]);
        assert!(detect(&root).is_none());
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
