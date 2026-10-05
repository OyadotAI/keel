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
    // Never through a link. A cloned repository can ship `.keel` or `.keel/ignored.json` as a
    // symlink to anywhere, and a plain write followed it and replaced that file. The folders are
    // checked, and the file is written beside itself and renamed over: a rename replaces a link,
    // it does not follow one.
    crate::writes::no_link_under(repo, ".keel/ignored.json")?;
    let dir = repo.join(".keel");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?;
    let mut tmp = tempfile::NamedTempFile::new_in(&dir).map_err(|e| e.to_string())?;
    std::io::Write::write_all(&mut tmp, body.as_bytes()).map_err(|e| e.to_string())?;
    tmp.persist(path(repo))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(all(test, unix))]
mod link_tests {
    use super::*;

    /// A `.keel/ignored.json` that links outside the repository is replaced, not written through.
    #[test]
    fn setting_aside_never_writes_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8Path::from_path(dir.path()).unwrap().join("repo");
        let outside = Utf8Path::from_path(dir.path())
            .unwrap()
            .join("precious.txt");
        std::fs::create_dir_all(repo.join(".keel")).unwrap();
        std::fs::write(&outside, "keep me").unwrap();
        std::os::unix::fs::symlink(&outside, repo.join(".keel/ignored.json")).unwrap();
        set(&repo, "deploy/no-ci", true, "").unwrap();
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep me");
        assert!(ids(&repo).contains(&"deploy/no-ci".to_string()));

        // A linked `.keel` folder is refused outright.
        let other = Utf8Path::from_path(dir.path()).unwrap().join("other");
        std::fs::create_dir_all(other.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(other.join("elsewhere"), other.join(".keel")).unwrap();
        assert!(set(&other, "x", true, "").is_err());
    }
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
    // Set aside is not passed: the row leaves the list with its finding, and "Ignored" names it.
    out.checks
        .retain(|c| c.passed || !ignored.iter().any(|i| i == c.id));
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
