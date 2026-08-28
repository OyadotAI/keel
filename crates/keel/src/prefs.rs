//! What Keel remembers between launches.
//!
//! Only two things, and both exist for the same reason: an application launched from the Dock has
//! no working directory to infer a project from, and asking someone which folder they meant every
//! single time is not a product. So the last project is remembered, and whether onboarding has
//! been completed is remembered, and nothing else — everything about how a project is built lives
//! in that project, where it can be committed and reviewed.

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct Prefs {
    /// Reopened on the next launch. `None` before a project has ever been opened.
    #[serde(default)]
    pub last_project: Option<Utf8PathBuf>,
    /// Set once the welcome flow has been finished, so it never appears again.
    #[serde(default)]
    pub onboarded: bool,
    /// The window's last logical size. An application that opens at the same default size every
    /// morning, whatever you left it at, is one of the quieter ways software says it is not one.
    #[serde(default)]
    pub window: Option<(f64, f64)>,
}

/// `~/.keel`. Keel's own directory, deliberately separate from `~/.claude`: Keel reads Claude
/// Code's state and must never be a reason for it to change.
pub fn dir() -> Option<Utf8PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(Utf8PathBuf::from(home).join(".keel"))
}

fn path() -> Option<Utf8PathBuf> {
    Some(dir()?.join("state.json"))
}

impl Prefs {
    pub fn load() -> Self {
        let Some(p) = path() else {
            return Self::default();
        };
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            // A corrupt or half-written file is not worth an error dialog on launch. Starting from
            // defaults loses a remembered path; refusing to start loses the application.
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let (Some(d), Some(p)) = (dir(), path()) else {
            return;
        };
        if std::fs::create_dir_all(&d).is_err() {
            return;
        }
        if let Ok(body) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&p, body);
        }
    }

    /// The project to reopen, if it is still there.
    ///
    /// A remembered path that has since been moved or deleted is not an error to report — it is a
    /// reason to show the welcome screen, which is what someone in that position needs anyway.
    pub fn resume(&self) -> Option<Utf8PathBuf> {
        let p = self.last_project.as_ref()?;
        p.is_dir().then(|| p.clone())
    }

    pub fn remember(project: &Utf8Path) {
        let mut prefs = Self::load();
        prefs.last_project = Some(project.to_owned());
        prefs.onboarded = true;
        prefs.save();
    }

    /// Written when the window closes rather than on every resize — a drag is hundreds of events
    /// and none of them are worth a write.
    pub fn remember_window(width: f64, height: f64) {
        let mut prefs = Self::load();
        prefs.window = Some((width, height));
        prefs.save();
    }
}

/// The working directory used when no project is open.
///
/// Representing "nothing is open" as an empty directory rather than as an absent path is a
/// deliberate trade. Every handler already takes a repository and stays inside it; a `None` would
/// have to be answered at twenty-two call sites, and each answer is a chance to get it wrong. An
/// empty directory is true — nothing is open, so there is nothing to see — and it bounds anything
/// that reaches a handler before a project exists to a scratch folder that holds nothing.
pub fn no_project() -> Utf8PathBuf {
    let path = dir()
        .map(|d| d.join("no-project"))
        .unwrap_or_else(|| Utf8PathBuf::from("/tmp/keel-no-project"));
    let _ = std::fs::create_dir_all(&path);
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remembered_path_that_no_longer_exists_is_not_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let real = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();

        let prefs = Prefs {
            last_project: Some(real.clone()),
            onboarded: true,
            ..Default::default()
        };
        assert_eq!(prefs.resume(), Some(real.clone()));

        let gone = Prefs {
            last_project: Some(real.join("deleted-since")),
            onboarded: true,
            ..Default::default()
        };
        assert_eq!(gone.resume(), None, "a moved project shows the welcome screen");

        assert_eq!(Prefs::default().resume(), None);
    }

    /// Corrupt state on disk must not stop the application from starting.
    #[test]
    fn unreadable_state_falls_back_to_defaults() {
        let parsed: Prefs = serde_json::from_str("{ not json").unwrap_or_default();
        assert!(!parsed.onboarded);
        assert_eq!(parsed.last_project, None);
    }
}
