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
/// `None` is "Ask each lane", the default; `Some` is "Always …", which every lane without a
/// choice of its own follows and so never sees the card.
pub(super) fn save(selected: Option<Backend>, revision: u64) -> Result<Preference, String> {
    change(|record| {
        if record.revision != revision {
            return Err(
                "Browser preferences changed in another window. Refresh before saving.".into(),
            );
        }
        record.selected = selected;
        record.revision = record.revision.saturating_add(1);
        Ok(record.clone())
    })
}
/// Where one lane's choice lives: its own small file, so a thousand lanes never grow the device
/// record, keyed by the project's real path and the lane's id (the app's UUID, which every hook and
/// `/mcp` URL already carries as `?lane=`).
fn lane_path(path: &Utf8Path, project: &Utf8Path, lane: &str) -> Result<Utf8PathBuf, String> {
    use sha2::{Digest, Sha256};
    let root = path.parent().ok_or("Missing browser preference folder")?;
    let key = format!("{project}\n{lane}");
    let name = format!("browser-lanes/{:x}.json", Sha256::digest(key.as_bytes()));
    crate::writes::no_link_under(root, &name)?;
    let file = root.join(name);
    if std::fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("Browser lane preference is a symbolic link.".into());
    }
    Ok(file)
}
fn lane_choice(path: &Utf8Path) -> Result<Option<Backend>, String> {
    use std::io::Read;
    match std::fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(129)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 128 {
                return Err("Lane browser preference exceeds its limit.".into());
            }
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| "Couldn’t read this lane’s saved browser choice.".into())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// The browser this lane uses for its next call: its own choice, else the device's "Always …"
/// setting, else `None` — ask the person. Read on every call, so a switch in the Browser tab or a
/// card answered a moment ago applies to the very next step, in every session the lane runs.
pub(super) fn resolve(project: &Utf8Path, lane: &str) -> Result<Option<Backend>, String> {
    resolve_at(&path()?, project, lane)
}
fn resolve_at(path: &Utf8Path, project: &Utf8Path, lane: &str) -> Result<Option<Backend>, String> {
    Ok(lane_choice(&lane_path(path, project, lane)?)?.or(load(path)?.selected))
}

/// Remember `backend` for this lane, from the card or the Browser tab.
pub(super) fn set_lane(project: &Utf8Path, lane: &str, backend: Backend) -> Result<(), String> {
    set_lane_at(&path()?, project, lane, backend)
}
fn set_lane_at(
    path: &Utf8Path,
    project: &Utf8Path,
    lane: &str,
    backend: Backend,
) -> Result<(), String> {
    let file = lane_path(path, project, lane)?;
    crate::writes::locked(file.as_std_path(), || {
        crate::writes::replace(
            file.as_std_path(),
            &serde_json::to_vec(&backend).map_err(|e| e.to_string())?,
        )
    })?
}

/// Forget this lane's own choice, so its next browser step follows the device setting or asks.
pub(super) fn clear_lane(project: &Utf8Path, lane: &str) -> Result<(), String> {
    match std::fs::remove_file(lane_path(&path()?, project, lane)?) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

/// Only the lane's own choice, for the Browser tab's switch: a device default is not the lane's.
pub(super) fn lane_only(project: &Utf8Path, lane: &str) -> Result<Option<Backend>, String> {
    lane_choice(&lane_path(&path()?, project, lane)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Utf8PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(dir.path())
            .unwrap()
            .join("browser.json");
        (dir, path)
    }
    #[test]
    fn a_lane_choice_outlives_its_session_and_two_lanes_differ() {
        let (_dir, path) = fixture();
        let project = Utf8Path::new("/repo");
        set_lane_at(&path, project, "a", Backend::LocalOya).unwrap();
        set_lane_at(&path, project, "b", Backend::Embedded).unwrap();
        assert_eq!(
            resolve_at(&path, project, "a").unwrap(),
            Some(Backend::LocalOya)
        );
        assert_eq!(
            resolve_at(&path, project, "b").unwrap(),
            Some(Backend::Embedded)
        );
        assert_eq!(
            resolve_at(&path, Utf8Path::new("/other"), "a").unwrap(),
            None,
            "per project"
        );
        for index in 0..600 {
            set_lane_at(&path, project, &format!("lane-{index}"), Backend::LocalOya).unwrap();
        }
        assert_eq!(
            load(&path).unwrap().sessions.len(),
            0,
            "lanes never fill the device record"
        );
    }
    #[test]
    fn a_lane_with_no_choice_asks_unless_the_device_says_always() {
        let (_dir, path) = fixture();
        let project = Utf8Path::new("/repo");
        assert_eq!(
            resolve_at(&path, project, "new").unwrap(),
            None,
            "ask each lane by default"
        );
        change_at(&path, |r| {
            r.selected = Some(Backend::Embedded);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            resolve_at(&path, project, "new").unwrap(),
            Some(Backend::Embedded)
        );
        set_lane_at(&path, project, "new", Backend::LocalOya).unwrap();
        assert_eq!(
            resolve_at(&path, project, "new").unwrap(),
            Some(Backend::LocalOya),
            "the lane wins"
        );
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
