//! Device-local browser choice stays separate from credentials, project trust and live bindings.
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
const STORE_BYTES: usize = 128 * 1024;
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Backend {
    #[default]
    Embedded,
    LocalOya,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub(super) struct Preference {
    pub selected: Option<Backend>,
    pub revision: u64,
    pub sessions: BTreeMap<String, Backend>,
}
#[cfg(not(test))]
fn path() -> Result<Utf8PathBuf, String> {
    Ok(crate::prefs::dir()
        .ok_or("No home directory is available for browser preferences.")?
        .join("browser.json"))
}
#[cfg(test)]
fn path() -> Result<Utf8PathBuf, String> {
    static ROOT: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let root = ROOT.get_or_init(|| tempfile::tempdir().expect("scratch browser preferences"));
    Ok(Utf8Path::from_path(root.path())
        .ok_or("Non-UTF8 scratch path")?
        .join("browser.json"))
}

fn load(path: &Utf8Path) -> Result<Preference, String> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Preference::default()),
        Err(e) => return Err(e.to_string()),
    };
    let mut data = Vec::new();
    file.take((STORE_BYTES + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > STORE_BYTES {
        return Err("Browser preference storage exceeds its limit.".into());
    }
    serde_json::from_slice(&data).map_err(|_| {
        "Couldn’t read the saved browser choice. Existing preferences were preserved.".into()
    })
}
pub(super) fn read() -> Result<Preference, String> {
    load(&path()?)
}
fn change<T>(action: impl FnOnce(&mut Preference) -> Result<T, String>) -> Result<T, String> {
    change_at(&path()?, action)
}
fn change_at<T>(
    path: &Utf8Path,
    action: impl FnOnce(&mut Preference) -> Result<T, String>,
) -> Result<T, String> {
    crate::writes::locked(path.as_std_path(), || {
        let mut record = load(path)?;
        let result = action(&mut record)?;
        crate::writes::replace(
            path.as_std_path(),
            &serde_json::to_vec(&record).map_err(|e| e.to_string())?,
        )?;
        Ok(result)
    })?
}
pub(super) fn save(selected: Backend, revision: u64) -> Result<Preference, String> {
    change(|record| {
        if record.revision != revision {
            return Err(
                "Browser preferences changed in another window. Refresh before saving.".into(),
            );
        }
        record.selected = Some(selected);
        record.revision = record.revision.saturating_add(1);
        Ok(record.clone())
    })
}
fn key(project: &Utf8Path, provider: &str, session: &str) -> String {
    format!("{project}\n{provider}\n{session}")
}
fn session_path(path: &Utf8Path, key: &str) -> Result<Utf8PathBuf, String> {
    use sha2::{Digest, Sha256};
    let root = path.parent().ok_or("Missing browser preference folder")?;
    let name = format!("browser-sessions/{:x}.json", Sha256::digest(key.as_bytes()));
    crate::writes::no_link_under(root, &name)?;
    let file = root.join(name);
    if std::fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("Browser session preference is a symbolic link.".into());
    }
    Ok(file)
}
fn session_choice(path: &Utf8Path) -> Result<Option<Backend>, String> {
    use std::io::Read;
    match std::fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(129)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 128 {
                return Err("Session browser preference exceeds its limit.".into());
            }
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| "Couldn’t read the saved session browser choice.".into())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
fn store_session(path: &Utf8Path, backend: Backend) -> Result<(), String> {
    crate::writes::replace(
        path.as_std_path(),
        &serde_json::to_vec(&backend).map_err(|e| e.to_string())?,
    )
}
pub(super) fn capture(
    project: &Utf8Path,
    provider: &str,
    session: &str,
    resume: bool,
) -> Result<Backend, String> {
    let path = path()?;
    crate::writes::locked(path.as_std_path(), || {
        let record = load(&path)?;
        let key = key(project, provider, session);
        let session_path = session_path(&path, &key)?;
        if resume
            && let Some(backend) =
                session_choice(&session_path)?.or_else(|| record.sessions.get(&key).copied())
        {
            return Ok(backend);
        }
        let backend = record.selected.unwrap_or_default();
        store_session(&session_path, backend)?;
        Ok(backend)
    })?
}
pub(super) fn alias(
    project: &Utf8Path,
    provider: &str,
    session: &str,
    backend: Backend,
) -> Result<(), String> {
    let path = path()?;
    crate::writes::locked(path.as_std_path(), || {
        let session_path = session_path(&path, &key(project, provider, session))?;
        store_session(&session_path, backend)
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_choices_are_independent_and_do_not_fill_the_preference_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(dir.path())
            .unwrap()
            .join("browser.json");
        for index in 0..600 {
            let file = session_path(&path, &format!("session-{index}")).unwrap();
            store_session(&file, Backend::LocalOya).unwrap();
        }
        let original = session_path(&path, "session-0").unwrap();
        assert_eq!(session_choice(&original).unwrap(), Some(Backend::LocalOya));
        assert_eq!(load(&path).unwrap().sessions.len(), 0);
    }
    #[test]
    fn choice_and_session_binding_survive_reopening_without_storing_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(dir.path())
            .unwrap()
            .join("browser.json");
        change_at(&path, |record| {
            record.selected = Some(Backend::LocalOya);
            record.revision = 1;
            record
                .sessions
                .insert("project\ncodex\nsession".into(), Backend::Embedded);
            Ok(())
        })
        .unwrap();
        let reopened = load(&path).unwrap();
        assert_eq!(reopened.selected, Some(Backend::LocalOya));
        assert_eq!(
            reopened.sessions["project\ncodex\nsession"],
            Backend::Embedded
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.contains("Bearer"));
        assert!(!text.contains("Authorization"));
    }
    #[test]
    fn simultaneous_changes_keep_the_previous_choice_when_revision_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(dir.path())
            .unwrap()
            .join("browser.json");
        change_at(&path, |record| {
            record.selected = Some(Backend::Embedded);
            record.revision = 2;
            Ok(())
        })
        .unwrap();
        let stale = change_at(&path, |record| {
            if record.revision != 1 {
                return Err::<(), _>("changed".into());
            }
            record.selected = Some(Backend::LocalOya);
            Ok(())
        });
        assert!(stale.is_err());
        assert_eq!(load(&path).unwrap().selected, Some(Backend::Embedded));
    }
}
