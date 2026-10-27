//! Atomic known-session registry and serialized native name reconciliation.
use camino::{Utf8Path, Utf8PathBuf};
use keel_workspace::{NativeName, Session, SessionNames};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Identity {
    pub provider: String,
    pub id: String,
    pub repo: String,
    pub cwd: String,
    pub wt: Option<String>,
}
impl Identity {
    fn key(&self) -> String {
        format!("{}:{}", self.provider, self.id)
    }
}
#[derive(Clone, Deserialize, Serialize)]
struct Known {
    identity: Identity,
    title: Option<String>,
    source: Option<String>,
    fingerprint: String,
    revision: u64,
    pending: Option<String>,
    last_active: Option<String>,
    messages: usize,
}
type Records = BTreeMap<String, Known>;

pub(super) struct Repository {
    pub dir: Utf8PathBuf,
    pub native: SessionNames,
}
impl Repository {
    /// Resolve private storage roots once for the naming operation.
    pub fn local() -> Result<Self, String> {
        Ok(Self {
            dir: crate::prefs::dir().ok_or("No home directory")?,
            native: SessionNames::local().ok_or("No provider directories")?,
        })
    }
    fn path(&self) -> Utf8PathBuf {
        self.dir.join("known-sessions.json")
    }
    fn load(&self) -> Result<Records, String> {
        match std::fs::read_to_string(self.path()) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.to_string()),
        }
    }
    fn change<T>(&self, action: impl FnOnce(&mut Records) -> T) -> Result<T, String> {
        let path = self.path();
        crate::writes::locked(path.as_std_path(), || {
            let mut records = self.load()?;
            let before = serde_json::to_vec(&records).map_err(|e| e.to_string())?;
            let out = action(&mut records);
            let after = serde_json::to_vec(&records).map_err(|e| e.to_string())?;
            if before != after {
                crate::writes::replace(path.as_std_path(), &after)?;
            }
            Ok(out)
        })?
    }
    fn serialized<T>(
        &self,
        identity: &Identity,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let path = self
            .dir
            .join("session-name-locks")
            .join(format!("{}-{}", identity.provider, identity.id));
        crate::writes::locked(path.as_std_path(), action)?
    }
    fn remember(&self, identity: Identity) -> Result<Known, String> {
        self.change(|records| {
            let record = records.entry(identity.key()).or_insert_with(|| Known {
                identity: identity.clone(),
                title: None,
                source: None,
                fingerprint: String::new(),
                revision: 0,
                pending: None,
                last_active: None,
                messages: 0,
            });
            if record.identity.cwd == identity.cwd && identity.wt.is_some() {
                record.identity.wt = identity.wt;
            }
            record.clone()
        })
    }
    fn save(&self, record: &Known) -> Result<(), String> {
        self.change(|records| {
            records.insert(record.identity.key(), record.clone());
        })
    }
    /// Register a resumable identity and reconcile names under its cross-process lock.
    pub fn register(&self, identity: Identity) -> Result<Session, String> {
        self.serialized(&identity, || {
            let mut record = self.remember(identity.clone())?;
            if record.identity.cwd != identity.cwd {
                return Err("This session is already registered in another checkout".into());
            }
            self.refresh(&mut record)?;
            Ok(self.session(&record))
        })
    }
    /// Store intent before changing provider metadata so partial writes remain retryable.
    pub fn rename(&self, identity: Identity, name: &str, legacy: bool) -> Result<Session, String> {
        self.serialized(&identity, || {
            let mut record = self.remember(identity.clone())?;
            if record.identity.cwd != identity.cwd {
                return Err("This session belongs to another checkout".into());
            }
            self.refresh(&mut record)?;
            if legacy && record.source.as_deref() == Some("explicit") {
                return Ok(self.session(&record));
            }
            if record.pending.as_deref() != Some(name) {
                record.revision = record.revision.saturating_add(1);
            }
            record.pending = Some(name.into());
            self.save(&record)?;
            self.refresh(&mut record)?;
            Ok(self.session(&record))
        })
    }
    fn refresh(&self, record: &mut Known) -> Result<(), String> {
        let identity = &record.identity;
        let cwd = Utf8Path::new(&identity.cwd);
        let Some(mut native) = self.native.read(&identity.provider, cwd, &identity.id)? else {
            return Ok(());
        };
        if record.pending.as_ref().is_some_and(|name| {
            native.explicit
                && native.title.as_ref() != Some(name)
                && !record.fingerprint.is_empty()
                && native.fingerprint != record.fingerprint
        }) {
            record.pending = None;
        }
        if let Some(name) = &record.pending {
            if identity.provider == "codex" {
                super::codex_api::rename(cwd, &identity.id, name)?;
            } else {
                self.native
                    .rename(&identity.provider, cwd, &identity.id, name)?;
            }
            native = self
                .native
                .read(&identity.provider, cwd, &identity.id)?
                .ok_or("The renamed session could not be read back")?;
            if (!name.is_empty() && native.title.as_deref().unwrap_or_default() != name)
                || (name.is_empty() && native.explicit)
            {
                return Err("The provider did not persist the session name".into());
            }
            record.pending = None;
        }
        Self::observe(record, native);
        self.save(record)
    }
    fn observe(record: &mut Known, native: NativeName) {
        if record.fingerprint != native.fingerprint {
            record.revision = record.revision.saturating_add(1);
            record.fingerprint = native.fingerprint;
            record.title = native.title;
            record.source = Some(
                if native.explicit {
                    "explicit"
                } else {
                    "generated"
                }
                .into(),
            );
        }
        record.last_active = native.last_active;
        record.messages = native.messages;
    }
    fn session(&self, record: &Known) -> Session {
        let i = &record.identity;
        let mut s = Session::known(&i.provider, &i.id, &i.cwd);
        s.wt = i.wt.clone();
        s.title = record.pending.clone().or_else(|| record.title.clone());
        s.title_source = if record.pending.is_some() {
            Some("pending")
        } else {
            record.source.as_deref().map(|s| {
                if s == "explicit" {
                    "explicit"
                } else {
                    "generated"
                }
            })
        };
        s.title_revision = record.revision;
        s.last_active = record.last_active.clone();
        s.messages = record.messages;
        s
    }
    /// Merge native Claude discovery with registered provider sessions through one resolver.
    pub fn list(&self, repo: &Utf8Path, mut raw: Vec<Session>) -> Result<Vec<Session>, String> {
        for s in &mut raw {
            let identity = Identity {
                provider: "claude".into(),
                id: s.id.clone(),
                repo: repo.to_string(),
                cwd: s.cwd.clone().unwrap_or_else(|| repo.to_string()),
                wt: s
                    .cwd
                    .as_deref()
                    .and_then(|cwd| worktree(repo, Utf8Path::new(cwd))),
            };
            if let Ok(named) = self.register(identity) {
                s.wt = named.wt;
                s.title = named.title;
                s.title_source = named.title_source;
                s.title_revision = named.title_revision;
            }
        }
        for record in self
            .load()?
            .values()
            .filter(|r| r.identity.repo == repo && r.identity.provider != "claude")
        {
            match self.register(record.identity.clone()) {
                Ok(s) => raw.push(s),
                Err(_) => raw.push(self.session(record)),
            }
        }
        self.legacy(&mut raw);
        raw.sort_by(|a, b| b.last_active.cmp(&a.last_active));
        Ok(raw)
    }
    fn legacy(&self, sessions: &mut [Session]) {
        let names: BTreeMap<String, String> =
            std::fs::read_to_string(self.dir.join("session-names.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
        for session in sessions.iter_mut().filter(|s| {
            s.provider == "claude"
                && s.title_source != Some("explicit")
                && s.title_source != Some("pending")
        }) {
            if let Some(name) = names.get(&session.id) {
                session.title = Some(name.clone());
                session.title_source = Some("legacy");
            }
        }
    }
}

/// Restore only exact Keel worktree roots, using the existing branch-name guard.
fn worktree(repo: &Utf8Path, cwd: &Utf8Path) -> Option<String> {
    let path = cwd.strip_prefix(repo.join(crate::worktree::DIR)).ok()?;
    let name = path.as_str();
    crate::worktree::valid_name(name).then(|| name.to_owned())
}
