//! Project config Keel itself wrote, so quarantine can tell it from config a repository shipped.
//!
//! Plugins and MCP servers install into the project — `enabledPlugins` in `.claude/settings.json`,
//! servers in `.mcp.json` — so a teammate who clones the repository gets them. Those are exactly
//! the files `keel_harness::trust` moves aside before every turn, because a repository's copy is
//! arbitrary code by whoever wrote it. The two meet here: when Keel writes one of them on the
//! person's click, it records the SHA-256 of the bytes it left, in `~/.keel` — this machine,
//! never the repository. Quarantine leaves a file alone only when its bytes match a record, so:
//!
//! - what the person installed here keeps working, turn after turn;
//! - a teammate's clone has no record, so their Keel quarantines it and asks, as for any repo;
//! - an edit to the file after Keel wrote it — by anyone — changes the hash and is quarantined.
//!
//! And a record is refused for content Keel would never write: a `settings.json` holding
//! anything but plugin keys (no `hooks`, `statusLine`, `apiKeyHelper`, `env`, …), an `.mcp.json`
//! holding anything but `mcpServers`. `.claude/hooks/` and `settings.local.json` are never vouched.

use camino::{Utf8Path, Utf8PathBuf};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// The project files Keel writes on the person's behalf, with the top-level keys each may hold.
const VOUCHABLE: &[(&str, &[&str])] = &[
    (
        ".claude/settings.json",
        &["$schema", "enabledPlugins", "extraKnownMarketplaces"],
    ),
    (".mcp.json", &["mcpServers"]),
];

fn store() -> Option<Utf8PathBuf> {
    Some(crate::prefs::dir()?.join("vouched.json"))
}

/// Record the file at `root/rel` as Keel's own, as it stands now. Errors name why not.
pub fn vouch(root: &Utf8Path, rel: &str) -> Result<(), String> {
    let store = store().ok_or("no home directory to keep the record in")?;
    vouch_in(&store, root, rel)
}

/// Whether `bytes`, found at `rel` in `root`, are exactly what Keel wrote there on this machine.
pub fn holds(root: &Utf8Path, rel: &str, bytes: &[u8]) -> bool {
    store().is_some_and(|s| holds_in(&s, root, rel, bytes))
}

/// The project a tree belongs to, for keying records: its resolved path, with a lane's checkout
/// (`<root>/.keel/worktrees/<name>`) counted as its project's. Keyed by project because a
/// record for identical bytes elsewhere is not a decision about this repository — a stdio
/// server's `npx foo` resolves inside whichever tree it runs in.
fn owner(root: &Utf8Path) -> Option<Utf8PathBuf> {
    let real =
        Utf8PathBuf::from_path_buf(keel_workspace::real_std(root.as_std_path()).ok()?).ok()?;
    let parent = real.parent()?;
    if parent.file_name() == Some("worktrees")
        && parent.parent().and_then(Utf8Path::file_name) == Some(".keel")
    {
        return parent.parent()?.parent().map(Utf8Path::to_owned);
    }
    Some(real)
}

fn vouch_in(store: &Utf8Path, root: &Utf8Path, rel: &str) -> Result<(), String> {
    let path = root.join(rel);
    let owner = owner(root).ok_or("the project folder could not be resolved")?;
    if std::fs::symlink_metadata(root.join(".claude")).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(".claude is a link — not recorded".into());
    }
    // A link is never Keel's write: it could point anywhere, and its target can change.
    let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("{rel}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{rel} is not a plain file"));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("{rel}: {e}"))?;
    shaped(rel, &bytes)?;
    // A file lock: the app's daemon and a `keel serve` from a terminal share this store.
    crate::writes::locked(store.as_std_path(), || {
        let mut all = read(store);
        all.insert(digest(&owner, rel, &bytes));
        let body = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
        let tmp = store.with_extension("json.tmp");
        std::fs::write(&tmp, body).map_err(|e| format!("{tmp}: {e}"))?;
        std::fs::rename(&tmp, store).map_err(|e| format!("{store}: {e}"))
    })?
}

/// Before the CLI writes `rel` at project scope: move aside whatever Keel did not write there, so
/// the install lands in a fresh file (or in Keel's own) and never merges into a repository's —
/// which would then be vouched along with it.
pub fn prepare(root: &Utf8Path) -> Result<(), String> {
    keel_harness::quarantine_keeping(root, |rel, bytes| holds(root, rel, bytes))
        .map(|_| ())
        .map_err(|e| format!("could not set aside the project's own agent config: {e:#}"))
}

/// After the CLI wrote `rel`: record it, and commit exactly it when asked. Returns the line to
/// show — what happened to the file, in words.
pub fn settle(root: &Utf8Path, rel: &str, commit: bool, message: &str) -> String {
    if std::fs::symlink_metadata(root.join(rel)).is_err() {
        return format!("{rel} is gone — nothing to record.");
    }
    if let Err(why) = vouch(root, rel) {
        return format!("{why}. It will be quarantined before the next turn.");
    }
    if !commit {
        return format!("Wrote {rel} in this project — not committed.");
    }
    match crate::writes::commit_only(root, &[rel.to_string()], message) {
        crate::writes::Committed { sha: Some(sha), .. } => format!("Committed {rel} ({sha})."),
        crate::writes::Committed { note, .. } => format!(
            "Wrote {rel} in this project — {}",
            note.unwrap_or_else(|| "not committed".into())
        ),
    }
}

fn holds_in(store: &Utf8Path, root: &Utf8Path, rel: &str, bytes: &[u8]) -> bool {
    shaped(rel, bytes).is_ok()
        && owner(root).is_some_and(|o| read(store).contains(&digest(&o, rel, bytes)))
}

/// Refuse anything outside the keys Keel writes. Checked on reading as well as on recording, so
/// a record can never cover content that would not have been allowed in.
fn shaped(rel: &str, bytes: &[u8]) -> Result<(), String> {
    let Some((_, keys)) = VOUCHABLE.iter().find(|(r, _)| *r == rel) else {
        return Err(format!("{rel} is not a file Keel writes"));
    };
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| format!("{rel} is not valid JSON"))?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("{rel} is not a JSON object"))?;
    match object.keys().find(|k| !keys.contains(&k.as_str())) {
        Some(extra) => Err(format!(
            "{rel} holds `{extra}`, which Keel does not write — left to quarantine"
        )),
        None => Ok(()),
    }
}

/// The project and the path are in the hash, so identical bytes anywhere else are not covered.
fn digest(owner: &Utf8Path, rel: &str, bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(owner.as_str().as_bytes());
    h.update([0]);
    h.update(rel.as_bytes());
    h.update([0]);
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// A missing or unreadable store is an empty one: the safe reading is "Keel wrote nothing".
fn read(store: &Utf8Path) -> BTreeSet<String> {
    std::fs::read(store)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// `claude <args>` in the repository, streaming its output; when `project`, as Keel's write of
/// `rel` — under the tree's writer claim, the repository's own copy set aside first, the result
/// recorded (and committed when asked) after. Returns the exit code.
pub async fn run(
    state: &std::sync::Arc<crate::serve::AppState>,
    rel: &'static str,
    project: bool,
    args: &[String],
    commit: bool,
    message: &str,
    tx: &tokio::sync::mpsc::Sender<Result<axum::response::sse::Event, std::convert::Infallible>>,
) -> i32 {
    use axum::response::sse::Event;
    use tokio::process::Command;
    let say = |line: String| {
        let tx = tx.clone();
        async move {
            let _ = tx.send(Ok(Event::default().event("line").data(line))).await;
        }
    };
    say(format!("$ claude {}", args.join(" "))).await;
    // In the repository either way: Claude Code decides which project a scope means from the
    // working directory, and the Dock launches Keel in `/`.
    let root = state.repo();
    let claude = || {
        let mut c = Command::new(crate::permissions::program("claude"));
        c.current_dir(&root).args(args);
        c
    };
    if !project {
        return crate::plugins::pipe(&mut claude(), tx).await;
    }
    let held = match crate::writes::hold(state, &root, "project-config") {
        Ok(h) => h,
        Err((_, why)) => {
            say(why).await;
            return 1;
        }
    };
    let prep = root.clone();
    if let Ok(Err(why)) = tokio::task::spawn_blocking(move || prepare(&prep)).await {
        say(why).await;
        return 1;
    }
    let code = crate::plugins::pipe(&mut claude(), tx).await;
    let (at, message) = (root.clone(), message.to_string());
    let line = tokio::task::spawn_blocking(move || settle(&at, rel, commit, &message))
        .await
        .unwrap_or_else(|_| format!("Could not record {rel}."));
    drop(held);
    say(line).await;
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(files: &[(&str, &str)]) -> (tempfile::TempDir, Utf8PathBuf, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let base = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let root = base.join("repo");
        for (rel, body) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        (dir, root, base.join("home/vouched.json"))
    }

    const PLUGINS: &str = r#"{"enabledPlugins":{"github@claude-plugins-official":true}}"#;

    #[test]
    fn what_keel_wrote_is_held_and_nothing_else_is() {
        let (_d, root, store) = setup(&[(".claude/settings.json", PLUGINS)]);
        assert!(!holds_in(
            &store,
            &root,
            ".claude/settings.json",
            PLUGINS.as_bytes()
        ));
        vouch_in(&store, &root, ".claude/settings.json").unwrap();
        assert!(holds_in(
            &store,
            &root,
            ".claude/settings.json",
            PLUGINS.as_bytes()
        ));
        // One byte different is someone else's file.
        let edited = PLUGINS.replace("true", "false");
        assert!(!holds_in(
            &store,
            &root,
            ".claude/settings.json",
            edited.as_bytes()
        ));
        // The same bytes at a path Keel did not write are not covered.
        assert!(!holds_in(
            &store,
            &root,
            ".claude/settings.local.json",
            PLUGINS.as_bytes()
        ));
    }

    #[test]
    fn a_hook_is_never_vouched() {
        let hooked = r#"{"enabledPlugins":{},"hooks":{"SessionStart":[]}}"#;
        let (_d, root, store) = setup(&[(".claude/settings.json", hooked)]);
        let why = vouch_in(&store, &root, ".claude/settings.json").unwrap_err();
        assert!(why.contains("hooks"), "{why}");
        assert!(!holds_in(
            &store,
            &root,
            ".claude/settings.json",
            hooked.as_bytes()
        ));
    }

    #[test]
    fn an_mcp_file_holds_servers_and_nothing_else() {
        let ok = r#"{"mcpServers":{"linear":{"type":"http","url":"https://mcp.linear.app/mcp"}}}"#;
        let (_d, root, store) = setup(&[(".mcp.json", ok)]);
        vouch_in(&store, &root, ".mcp.json").unwrap();
        assert!(holds_in(&store, &root, ".mcp.json", ok.as_bytes()));
        assert!(shaped(".mcp.json", br#"{"mcpServers":{},"hooks":{}}"#).is_err());
    }

    /// The same bytes in another repository are that repository's, not a decision already made.
    #[test]
    fn a_record_covers_its_own_project_and_its_lanes_only() {
        let (_d, root, store) = setup(&[(".mcp.json", r#"{"mcpServers":{}}"#)]);
        vouch_in(&store, &root, ".mcp.json").unwrap();
        let bytes = br#"{"mcpServers":{}}"#;
        let other = root.parent().unwrap().join("other");
        let lane = root.join(".keel/worktrees/fix-login");
        for d in [&other, &lane] {
            std::fs::create_dir_all(d).unwrap();
        }
        assert!(!holds_in(&store, &other, ".mcp.json", bytes));
        assert!(
            holds_in(&store, &lane, ".mcp.json", bytes),
            "a lane is its project"
        );
    }

    #[test]
    fn hooks_and_local_settings_are_not_vouchable() {
        let (_d, root, store) = setup(&[(".claude/settings.local.json", PLUGINS)]);
        assert!(vouch_in(&store, &root, ".claude/settings.local.json").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_never_vouched() {
        let (_d, root, store) = setup(&[("elsewhere.json", PLUGINS)]);
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::os::unix::fs::symlink(
            root.join("elsewhere.json"),
            root.join(".claude/settings.json"),
        )
        .unwrap();
        assert!(vouch_in(&store, &root, ".claude/settings.json").is_err());
    }
}
