//! Private fail-closed control state serializes durable intent before execution side effects.
use super::{constants::*, model::*};
use camino::Utf8Path;
use sha2::{Digest, Sha256};

pub(crate) struct Repository {
    pub(super) key: String,
    #[cfg(test)]
    fail_writes: std::sync::atomic::AtomicBool,
}

/// Conflicting receipt intent is a client conflict; unavailable storage remains retryable uncertainty.
#[derive(Debug)]
pub(crate) enum ReserveError {
    Invalid,
    Conflict,
    Storage(String),
}
impl Repository {
    /// The existing trust identity prevents replacement clones from inheriting enabled schedules.
    pub(crate) fn new(project: &Utf8Path) -> Result<Self, String> {
        use keel_workspace::Real;
        let key = crate::permissions::key_of(project)?;
        let dir = crate::permissions::local_path(AREA, &format!("{key}.json"))?;
        if dir.real().unwrap_or(dir.clone()).starts_with(project) {
            return Err("Execution authority cannot be stored inside the project.".into());
        }
        Ok(Self {
            key,
            #[cfg(test)]
            fail_writes: Default::default(),
        })
    }

    fn filename(&self) -> String {
        format!("{}.json", self.key)
    }
    fn marker(&self) -> String {
        format!("{}.bound", self.key)
    }
    fn fresh(&self) -> Store {
        Store {
            version: VERSION,
            binding: self.key.clone(),
            revision: 0,
            receipts: Default::default(),
            schedules: Default::default(),
            occurrences: Default::default(),
            active: None,
        }
    }

    fn load(&self) -> Result<Store, String> {
        let marker = crate::permissions::read_local(AREA, &self.marker(), STORE_BYTES)?;
        let Some(bytes) = crate::permissions::read_local(AREA, &self.filename(), STORE_BYTES)?
        else {
            return if marker.is_some() {
                Err("Saved execution state is missing. Execution is paused; restore the existing state before retrying.".into())
            } else {
                Ok(self.fresh())
            };
        };
        if marker.as_deref() != Some(self.key.as_bytes()) {
            return Err(
                "Saved execution binding is missing or damaged. Execution is paused.".into(),
            );
        }
        let store: Store = serde_json::from_slice(&bytes).map_err(|_| "Saved execution state is unreadable. Execution is paused and existing bytes were preserved.")?;
        if store.version != VERSION || store.binding != self.key {
            return Err(
                "Saved execution state has an unsupported version or repository identity.".into(),
            );
        }
        validate(&store)?;
        Ok(store)
    }

    /// Readers share the same short lease as writers so no partial control decision is observable.
    pub(crate) fn read(&self) -> Result<Store, String> {
        self.locked(|| self.load())
    }

    fn locked<T>(&self, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let name = format!("execution-{}.lock", self.key);
        locked_named(&name, action)
    }

    /// Mutations preserve the previous state on validation errors and sync receipts before returning.
    pub(crate) fn change<T>(
        &self,
        action: impl FnOnce(&mut Store) -> Result<T, String>,
    ) -> Result<T, String> {
        #[cfg(test)]
        if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("Controlled execution write failure.".into());
        }
        self.locked(|| {
            let mut store = self.load()?;
            let result = action(&mut store)?;
            store.revision = store
                .revision
                .checked_add(1)
                .ok_or("Execution revision is exhausted.")?;
            validate(&store)?;
            let bytes = serde_json::to_vec(&store).map_err(|e| e.to_string())?;
            if bytes.len() as u64 > STORE_BYTES {
                return Err("Execution state is full; existing receipts were preserved.".into());
            }
            crate::permissions::replace_local(AREA, &self.marker(), self.key.as_bytes())?;
            crate::permissions::replace_local(AREA, &self.filename(), &bytes)?;
            Ok(result)
        })
    }

    /// Explicit recovery reserves room for a new bounded operation and verifies atomic write access.
    pub(crate) fn check_capacity(&self) -> Result<(), String> {
        self.change(|store| {
            let bytes = serde_json::to_vec(store).map_err(|e| e.to_string())?.len() as u64;
            if store.receipts.len() >= RECEIPT_LIMIT || store.occurrences.len() >= OCCURRENCE_LIMIT || bytes.saturating_add(RETRY_HEADROOM_BYTES) > STORE_BYTES {
                return Err("Execution history is full. Existing records were preserved; restore capacity before retrying recurring dispatch.".into());
            }
            Ok(())
        })
    }

    #[cfg(test)]
    /// Inject a private write failure without changing another test's shared host storage directory.
    pub(crate) fn block_writes(&self, blocked: bool) {
        self.fail_writes
            .store(blocked, std::sync::atomic::Ordering::SeqCst);
    }

    /// Reusing an operation ID may recover its original inputs, but can never replace them.
    pub(crate) fn reserve(
        &self,
        id: &str,
        kind: &str,
        input: serde_json::Value,
        now: u64,
    ) -> Result<Receipt, ReserveError> {
        if !valid_id(id) {
            return Err(ReserveError::Invalid);
        }
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&input).map_err(|e| ReserveError::Storage(e.to_string()))?
            )
        );
        let mut conflict = false;
        let result = self.change(|store| {
            if let Some(old) = store.receipts.get(id) {
                return if old.kind == kind && old.fingerprint == fingerprint {
                    Ok(old.clone())
                } else {
                    conflict = true;
                    Err("This receipt was already used for different inputs.".into())
                };
            }
            let receipt = Receipt {
                id: id.into(),
                kind: kind.into(),
                task: None,
                fingerprint,
                input,
                phase: Phase::Reserved,
                result: None,
                error: None,
                created_at: now,
                updated_at: now,
            };
            store.receipts.insert(id.into(), receipt.clone());
            Ok(receipt)
        });
        result.map_err(|message| {
            if conflict {
                ReserveError::Conflict
            } else {
                ReserveError::Storage(message)
            }
        })
    }
}

/// Storage families share bounded leases while keeping dispatcher recovery independent of history size.
pub(super) fn locked_named<T>(
    name: &str,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    for attempt in 0..LOCK_ATTEMPTS {
        if let Some(_lease) = crate::permissions::try_lease(name)? {
            return action();
        }
        if attempt + 1 < LOCK_ATTEMPTS {
            std::thread::sleep(LOCK_RETRY);
        }
    }
    Err("Execution state is busy. Retry the same operation.".into())
}

/// Receipt IDs are opaque components, never user-selected filesystem paths or shell arguments.
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

fn validate(store: &Store) -> Result<(), String> {
    if store.receipts.len() > RECEIPT_LIMIT
        || store.schedules.len() > SCHEDULE_LIMIT
        || store.occurrences.len() > OCCURRENCE_LIMIT
    {
        return Err("Execution history is full; no saved action was removed.".into());
    }
    if store
        .active
        .as_ref()
        .is_some_and(|id| !store.occurrences.contains_key(id))
    {
        return Err("Execution active-slot state is inconsistent. Execution is paused.".into());
    }
    if store
        .receipts
        .iter()
        .any(|(id, receipt)| id != &receipt.id || !valid_id(id))
    {
        return Err("Execution storage contains an invalid receipt binding.".into());
    }
    Ok(())
}
