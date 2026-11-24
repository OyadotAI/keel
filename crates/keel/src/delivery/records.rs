//! Revisioned metadata changes preserve ownership and pin referenced task notes.
use super::service::id;
use super::{Failure, IssuePreview, PrForm, Provenance, Record, Service};
use crate::tasks::Source;
use camino::Utf8Path;
impl Service {
    pub(super) fn record(&self, id: &str) -> Result<Record, Failure> {
        if let Some(record) = self
            .records
            .read()
            .map_err(Failure::Unavailable)?
            .records
            .get(id)
        {
            return Ok(record.clone());
        }
        let task = crate::tasks::get(&self.root, id).map_err(Failure::Missing)?;
        let base = crate::pr::base_for(&self.root, Utf8Path::new(&task.owner.checkout));
        let guarded = task.continuation.is_some();
        let mut record = self.empty_record(
            id,
            task.objective,
            task.owner,
            Provenance::Conversation,
            (base, String::new(), String::new()),
        );
        record.ownership_guard |= guarded;
        Ok(record)
    }

    pub(super) fn empty_record(
        &self,
        id: &str,
        title: String,
        owner: Source,
        source: Provenance,
        branch: (String, String, String),
    ) -> Record {
        blank_record(id, title, owner, source, branch, self.clock.now())
    }

    pub(super) fn change<T>(
        &self,
        id: &str,
        revision: u64,
        action: impl FnOnce(&mut Record) -> Result<T, String>,
    ) -> Result<T, Failure> {
        let _acceptance =
            crate::permissions::acceptance_lease(&self.root).map_err(Failure::Conflict)?;
        let initial = self.record(id)?;
        self.owner(id)?;
        if !self
            .records
            .read()
            .map_err(Failure::Unavailable)?
            .records
            .contains_key(id)
            && let Err(error) = crate::tasks::pin(&self.root, id)
            && !crate::tasks::get(&self.root, id).is_ok_and(|task| task.pinned)
        {
            return Err(Failure::Unavailable(error));
        }
        let mut conflict = false;
        let result = self.records.change(|store| {
            let record = store.records.entry(id.into()).or_insert(initial);
            if record.revision != revision {
                conflict = true;
                return Err(
                    "Task changed. Refresh its current revision; your draft was retained.".into(),
                );
            }
            let output = action(record)?;
            record.revision = record
                .revision
                .checked_add(1)
                .ok_or("Task revision is exhausted.")?;
            record.updated_at = self.clock.now();
            Ok(output)
        });
        result.map_err(|message| {
            if conflict {
                Failure::Conflict(message)
            } else {
                Failure::Unavailable(message)
            }
        })
    }

    /// Preview saves only read-only source data; no worktree or provider starts here.
    pub(crate) fn preview(
        &self,
        url: &str,
        reader: &mut impl crate::github_read::Read,
        project: &crate::github_read::Project,
    ) -> Result<IssuePreview, Failure> {
        let preview = crate::github_read::preview(reader, project, url, id()?, self.clock.now())
            .map_err(Failure::Invalid)?;
        self.records
            .change(|store| {
                store.previews.insert(preview.id.clone(), preview.clone());
                Ok(())
            })
            .map_err(Failure::Unavailable)?;
        Ok(preview)
    }
}

pub(super) fn blank_record(
    id: &str,
    title: String,
    owner: Source,
    source: Provenance,
    branch: (String, String, String),
    at: u64,
) -> Record {
    let (base, base_sha, branch) = branch;
    let ownership_guard = owner.wt.is_some();
    Record {
        task: id.into(),
        revision: 0,
        title,
        source,
        owner,
        base,
        base_sha,
        branch,
        ownership_guard,
        created_at: at,
        updated_at: at,
        corrections: vec![],
        checks: vec![],
        reviews: vec![],
        feedback: None,
        publication: None,
        form: PrForm::default(),
    }
}
