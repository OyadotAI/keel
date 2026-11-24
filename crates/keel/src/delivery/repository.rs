//! Strict atomic delivery storage preserves records and rejects corruption instead of resetting history.
use super::{constants::*, model::Store};
use camino::{Utf8Path, Utf8PathBuf};
use keel_workspace::Real;
use std::io::{Read, Write};

pub(crate) struct Repository {
    root: Utf8PathBuf,
}
impl Repository {
    /// Canonical project binding prevents a caller from choosing another project's delivery file.
    pub(crate) fn new(root: &Utf8Path) -> Result<Self, String> {
        Ok(Self {
            root: root.real().map_err(|e| e.to_string())?,
        })
    }

    fn path(&self) -> Result<Utf8PathBuf, String> {
        let mut at = self.root.clone();
        for part in [".keel", "delivery", "state.json"] {
            at.push(part);
            if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(
                    "Delivery storage contains a symbolic link. Existing records were preserved."
                        .into(),
                );
            }
        }
        Ok(at)
    }

    fn load(&self, path: &Utf8Path) -> Result<Store, String> {
        let marker = path.with_extension("bound");
        let binding = match std::fs::symlink_metadata(&marker) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
                return Err("Delivery binding is not a regular file.".into());
            }
            Ok(_) => {
                let mut bytes = Vec::new();
                std::fs::File::open(&marker)
                    .and_then(|f| {
                        f.take(BINDING_MARKER.len() as u64 + 1)
                            .read_to_end(&mut bytes)
                    })
                    .map_err(|e| e.to_string())?;
                Some(bytes)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        let file =
            match std::fs::File::open(path) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && binding.is_none() => {
                    return Ok(Store {
                        version: VERSION,
                        project: self.root.to_string(),
                        revision: 0,
                        records: Default::default(),
                        previews: Default::default(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(
                    "Saved delivery records are missing. Restore them before changing this task."
                        .into(),
                ),
                Err(e) => return Err(e.to_string()),
            };
        if binding.as_deref() != Some(BINDING_MARKER) {
            return Err(
                "Delivery binding is missing or unreadable. Existing records were preserved."
                    .into(),
            );
        }
        let mut body = Vec::new();
        file.take(STORE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|e| e.to_string())?;
        if body.len() as u64 > STORE_BYTES {
            return Err("Delivery storage exceeds its size limit.".into());
        }
        let store: Store = serde_json::from_slice(&body).map_err(
            |_| "Delivery storage could not be decoded. Existing records were preserved.",
        )?;
        if store.version != VERSION || store.project != self.root.as_str() {
            return Err("Delivery storage has an unsupported version or project binding.".into());
        }
        validate(&store)?;
        Ok(store)
    }

    /// Read failures remain visible so missing evidence never appears as an empty healthy task list.
    pub(super) fn read(&self) -> Result<Store, String> {
        self.load(&self.path()?)
    }

    /// A short nonblocking file lease serializes revisions across daemon processes without holding I/O work.
    pub(super) fn change<T>(
        &self,
        action: impl FnOnce(&mut Store) -> Result<T, String>,
    ) -> Result<T, String> {
        let path = self.path()?;
        let dir = path.parent().ok_or("Delivery storage has no parent.")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let _ = self.path()?;
        let key = crate::permissions::key_of(&self.root)?;
        let _lock = crate::permissions::try_lease(&format!("delivery-{key}.lock"))?
            .ok_or("Delivery storage is busy. Retry the same request.")?;
        self.ignore()?;
        let mut store = self.load(&path)?;
        let result = action(&mut store)?;
        store.revision = store
            .revision
            .checked_add(1)
            .ok_or("Delivery revision is exhausted.")?;
        validate(&store)?;
        let bytes = serde_json::to_vec(&store).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > STORE_BYTES {
            return Err("Delivery storage is full. Existing records were preserved.".into());
        }
        let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        tmp.write_all(&bytes)
            .and_then(|_| tmp.as_file().sync_all())
            .map_err(|e| e.to_string())?;
        let _ = self.path()?;
        crate::writes::replace(path.with_extension("bound").as_std_path(), BINDING_MARKER)?;
        tmp.persist(&path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        std::fs::File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(result)
    }

    fn ignore(&self) -> Result<(), String> {
        crate::writes::ignore_metadata(&self.root, &["delivery/"])
    }
}

fn validate(store: &Store) -> Result<(), String> {
    if store.records.len() > TASK_LIMIT || store.previews.len() > PREVIEW_LIMIT {
        return Err("Delivery storage has reached its record limit; nothing was removed.".into());
    }
    for (id, record) in &store.records {
        if id != &record.task
            || record.corrections.len() > CORRECTION_LIMIT
            || !Utf8Path::new(&record.owner.checkout).starts_with(&store.project)
        {
            return Err(
                "Delivery storage contains an invalid task binding or oversized correction list."
                    .into(),
            );
        }
    }
    Ok(())
}
