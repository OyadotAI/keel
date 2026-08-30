//! Findings the team has decided not to act on.
//!
//! A report with three items nobody will ever fix is a report people stop reading, so a
//! finding can be set aside — by id, in `.keel/ignored.json`, beside the permissions. In the
//! repository rather than in a preference, because it is a decision about this project that
//! the next person should see and be able to argue with.
//!
//! `keel scan` is deliberately unaffected: CI asked a question and gets the whole answer. Only
//! the panel hides them, and it says how many and offers them back.

use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Default, Serialize, Deserialize)]
struct Store {
    /// Finding id → why, or an empty string.
    #[serde(default)]
    ignored: BTreeMap<String, String>,
}

fn path(repo: &Utf8Path) -> camino::Utf8PathBuf {
    repo.join(".keel").join("ignored.json")
}

fn load(repo: &Utf8Path) -> Store {
    std::fs::read_to_string(path(repo))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn ids(repo: &Utf8Path) -> Vec<String> {
    load(repo).ignored.keys().cloned().collect()
}

pub fn set(repo: &Utf8Path, id: &str, ignored: bool, why: &str) -> Result<(), String> {
    let mut store = load(repo);
    if ignored {
        store.ignored.insert(id.to_string(), why.to_string());
    } else {
        store.ignored.remove(id);
    }
    let dir = repo.join(".keel");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?;
    std::fs::write(path(repo), body).map_err(|e| e.to_string())
}

/// The report as the panel should show it: ignored findings removed from the list and from
/// every phase, and named so the panel can offer them back.
pub fn apply(repo: &Utf8Path, report: keel_scanner::Report) -> keel_scanner::Report {
    let ignored = ids(repo);
    if ignored.is_empty() {
        return report;
    }
    let mut out = report;
    out.findings.retain(|f| !ignored.iter().any(|i| i == f.id));
    for phase in &mut out.plan {
        phase.findings.retain(|id| !ignored.iter().any(|i| i == id));
    }
    out.plan.retain(|p| !p.findings.is_empty());
    out.ignored = ignored;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ignored_finding_leaves_the_panel_but_is_still_named() {
        let dir = tempfile::tempdir().unwrap();
        let repo = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::write(repo.join("package.json"), "{}").unwrap();
        std::fs::write(repo.join("index.js"), "app.get('/x')").unwrap();
        let ctx = keel_scanner::RepoContext::load(&repo).unwrap();
        let full = keel_scanner::scan(&ctx);
        let id = full.findings.first().expect("something to ignore").id;
        set(&repo, id, true, "we know").unwrap();

        let shown = apply(&repo, keel_scanner::scan(&ctx));
        assert!(!shown.findings.iter().any(|f| f.id == id));
        assert!(shown.ignored.iter().any(|i| i == id));
        assert!(shown.plan.iter().all(|p| !p.findings.contains(&id)));
        // The score is the whole truth; hiding a finding does not earn points.
        assert_eq!(shown.score, full.score);
        // And it comes back.
        set(&repo, id, false, "").unwrap();
        assert!(
            apply(&repo, keel_scanner::scan(&ctx))
                .findings
                .iter()
                .any(|f| f.id == id)
        );
    }
}
