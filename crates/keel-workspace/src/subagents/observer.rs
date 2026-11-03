//! One parent stream owns incremental discovery; selected output uses only verified child links.
use super::{Availability, Detail, Invocation, Snapshot, Status, agy, claude, codex, json, output};
use crate::{Follows, Transcript};
use camino::{Utf8Path, Utf8PathBuf};
use std::collections::BTreeMap;

const OUTPUT_CACHE_LIMIT: usize = 16;

/// Injectable provider roots keep scratch transcript tests independent of real account state.
#[derive(Clone)]
pub struct Roots {
    pub codex_sessions: Utf8PathBuf,
}
impl Default for Roots {
    fn default() -> Self {
        Self {
            codex_sessions: crate::codex_home()
                .unwrap_or_else(|| "/nonexistent".into())
                .join("sessions"),
        }
    }
}

pub(super) struct Context {
    pub parent: Transcript,
    pub directory: Utf8PathBuf,
    pub id: String,
    pub roots: Roots,
}
pub(super) struct Entry {
    pub invocation: Invocation,
    pub path: Option<Utf8PathBuf>,
    pub root: Utf8PathBuf,
    pub provider: Follows,
    pub turn: Option<String>,
    pub version: String,
    pub confirmed: Status,
    pub result: Option<String>,
    pub current: bool,
    pub owner: Option<String>,
    pub directory: Option<Utf8PathBuf>,
    pub until: Option<String>,
    pub attributed: bool,
}
pub(super) type Entries = BTreeMap<String, Entry>;
pub(super) trait Adapter: Send {
    fn scan(&mut self, context: &Context, entries: &mut Entries) -> Option<bool>;
}

/// Observing never starts processes, submits messages, or changes parent completion decisions.
pub struct Observer {
    context: Context,
    adapter: Box<dyn Adapter>,
    entries: Entries,
    truncated: bool,
    cached_output: BTreeMap<String, (u64, Detail)>,
    minimum_started: Option<String>,
}
impl Observer {
    /// Production composition supplies account roots while retaining the authorized parent path.
    pub fn new(parent: Transcript, directory: Utf8PathBuf, id: String) -> Self {
        Self::with_roots(parent, directory, id, Roots::default())
    }

    /// Explicit roots support isolated provider fixtures without changing HOME or credentials.
    pub fn with_roots(
        parent: Transcript,
        directory: Utf8PathBuf,
        id: String,
        roots: Roots,
    ) -> Self {
        let adapter: Box<dyn Adapter> = match parent.agent() {
            Follows::Codex => Box::<codex::Reader>::default(),
            Follows::Claude => Box::<claude::Reader>::default(),
            Follows::Antigravity => Box::<agy::Reader>::default(),
        };
        Self {
            context: Context {
                parent,
                directory,
                id,
                roots,
            },
            adapter,
            entries: BTreeMap::new(),
            truncated: false,
            cached_output: BTreeMap::new(),
            minimum_started: None,
        }
    }

    /// A verified terminal restart bounds the lifetime whose unfinished work can still be current.
    pub fn lifetime(&mut self, since: Option<&str>) {
        if let Some(since) = since
            && self
                .minimum_started
                .as_deref()
                .is_none_or(|old| since > old)
        {
            self.minimum_started = Some(since.to_string());
        }
    }

    fn interrupt_previous_lifetime(&mut self) {
        let Some(since) = self.minimum_started.as_deref() else {
            return;
        };
        for entry in self
            .entries
            .values_mut()
            .filter(|e| e.invocation.status.active())
        {
            if let Some(started) = entry.invocation.started.as_deref() {
                if started < since {
                    entry.invocation.status = Status::Interrupted;
                    entry.confirmed = Status::Interrupted;
                    entry.current = false;
                }
            } else {
                entry.invocation.status = Status::Unknown;
            }
        }
    }

    /// Snapshot changes are display-only; missing data revokes current activity, never invents Done.
    pub fn snapshot(&mut self) -> Snapshot {
        self.interrupt_previous_lifetime();
        match self.adapter.scan(&self.context, &mut self.entries) {
            Some(truncated) => {
                self.truncated |= truncated;
                for entry in self.entries.values_mut() {
                    if entry.provider == Follows::Claude
                        && entry.invocation.child.is_none()
                        && entry.invocation.status == Status::Unknown
                        && entry.invocation.output != Availability::Disconnected
                    {
                        entry.invocation.status = entry.confirmed;
                    }
                }
            }
            None => {
                for entry in self
                    .entries
                    .values_mut()
                    .filter(|e| e.invocation.status.active())
                {
                    entry.invocation.status = Status::Unknown;
                }
            }
        }
        self.interrupt_previous_lifetime();
        let mut done: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, e)| !e.current || !e.invocation.status.active())
            .map(|(id, e)| {
                (
                    id.clone(),
                    e.invocation
                        .finished
                        .clone()
                        .or(e.invocation.started.clone())
                        .or_else(|| Some(e.version.clone())),
                )
            })
            .collect();
        done.sort_by(|a, b| a.1.cmp(&b.1));
        let remove = done.len().saturating_sub(json::HISTORY_LIMIT);
        for (id, _) in done.into_iter().take(remove) {
            self.entries.remove(&id);
            self.truncated = true;
        }
        let mut invocations: Vec<_> = self
            .entries
            .values()
            .map(|e| e.invocation.clone())
            .collect();
        invocations.sort_by(|a, b| a.started.cmp(&b.started).then(a.id.cmp(&b.id)));
        Snapshot {
            invocations,
            truncated: self.truncated || self.entries.len() >= json::CANDIDATE_LIMIT,
        }
    }

    /// Only a confirmed process exit may interrupt unfinished children; a socket close must not.
    pub fn interrupted(&mut self) -> Snapshot {
        let mut snapshot = self.snapshot();
        for invocation in &mut snapshot.invocations {
            if invocation.status.active() {
                invocation.status = Status::Interrupted;
            }
        }
        for entry in self.entries.values_mut() {
            if entry.invocation.status.active() {
                entry.invocation.status = Status::Interrupted;
                entry.confirmed = Status::Interrupted;
            }
        }
        snapshot
    }

    /// Lookup by invocation identity cannot expose an arbitrary child session or filesystem path.
    pub fn detail(&mut self, id: &str) -> Option<Detail> {
        self.snapshot();
        self.cached_detail(id)
    }

    /// A visible grid reads a bounded group after one lifecycle scan, without opening child PTYs.
    pub fn details(&mut self, ids: &[String]) -> BTreeMap<String, Detail> {
        self.snapshot();
        ids.iter()
            .take(super::OUTPUT_BATCH_LIMIT)
            .filter_map(|id| self.cached_detail(id).map(|detail| (id.clone(), detail)))
            .collect()
    }

    fn cached_detail(&mut self, id: &str) -> Option<Detail> {
        let entry = self.entries.get(id)?;
        if !output::authorized(entry) {
            return Some(Detail {
                items: vec![],
                availability: Availability::Disconnected,
                truncated: false,
            });
        }
        let stamp = output::signature(entry);
        if let Some((version, detail)) = self.cached_output.get(id)
            && *version == stamp
        {
            return Some(detail.clone());
        }
        let detail = output::read(entry);
        if self.cached_output.len() >= OUTPUT_CACHE_LIMIT
            && !self.cached_output.contains_key(id)
            && let Some(oldest) = self.cached_output.keys().next().cloned()
        {
            self.cached_output.remove(&oldest);
        }
        self.cached_output
            .insert(id.to_string(), (stamp, detail.clone()));
        Some(detail)
    }
}

pub(super) fn insert<'a>(
    entries: &'a mut Entries,
    id: String,
    child: Option<String>,
    member: Option<String>,
    provider: Follows,
    root: &Utf8Path,
    at: Option<String>,
) -> Option<&'a mut Entry> {
    if entries.len() >= json::CANDIDATE_LIMIT && !entries.contains_key(&id) {
        let retired = entries
            .iter()
            .filter(|(_, entry)| !entry.current || !entry.invocation.status.active())
            .min_by(|a, b| a.1.version.cmp(&b.1.version))
            .map(|(id, _)| id.clone())?;
        entries.remove(&retired);
    }
    Some(entries.entry(id.clone()).or_insert_with(|| Entry {
        invocation: Invocation {
            id,
            child,
            member,
            status: Status::Unknown,
            activity: None,
            started: at,
            finished: None,
            output: Availability::Waiting,
        },
        path: None,
        root: root.to_owned(),
        provider,
        turn: None,
        version: String::new(),
        confirmed: Status::Unknown,
        result: None,
        current: true,
        owner: None,
        directory: None,
        until: None,
        attributed: false,
    }))
}
pub(super) fn update(entry: &mut Entry, status: Status, at: Option<&str>) {
    let version = at.unwrap_or_default();
    if version < entry.version.as_str()
        || (version == entry.version && status == Status::Working && !entry.confirmed.active())
    {
        return;
    }
    entry.version = version.to_string();
    entry.invocation.status = status;
    entry.confirmed = status;
    entry.invocation.finished = (!status.active())
        .then(|| version.to_string())
        .filter(|v| !v.is_empty());
}
