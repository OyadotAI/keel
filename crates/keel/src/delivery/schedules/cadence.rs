//! Clock changes and missed intervals produce one history summary instead of a catch-up queue.
use super::{Failure, Occurrence, Phase, Schedule, Service, milliseconds};
use std::collections::BTreeMap;

/// Clock rollback or a long host gap reanchors cadence without changing active execution.
pub(super) fn shifted(previous: u64, now: u64) -> bool {
    now < previous || now.saturating_sub(previous) > super::super::constants::TICK_GAP_MS
}
impl Service {
    pub(super) fn reanchor(&self) -> Result<(), Failure> {
        let now = self.clock.now();
        if !self
            .execution
            .read()
            .map_err(Failure::Unavailable)?
            .schedules
            .values()
            .any(|schedule| schedule.enabled)
        {
            return Ok(());
        }
        self.execution
            .change(|store| {
                for schedule in store
                    .schedules
                    .values_mut()
                    .filter(|schedule| schedule.enabled)
                {
                    skip(
                        schedule,
                        &mut store.occurrences,
                        now,
                        "Clock or host wake changed the cadence; missed intervals were skipped.",
                    )?;
                }
                Ok(())
            })
            .map_err(Failure::Unavailable)?;
        super::changed();
        Ok(())
    }
    pub(super) fn advance_definitions(&self, restarted: bool) -> Result<Vec<String>, Failure> {
        let now = self.clock.now();
        let trust = crate::permissions::trust_stamp(&self.root);
        self.execution.change(|store|{
            let mut due=Vec::new();
            for schedule in store.schedules.values_mut().filter(|schedule|schedule.enabled){
                if trust.as_deref()!=Some(&schedule.trust){schedule.enabled=false;schedule.next_at=None;schedule.error=Some("Project trust changed. Explicitly Enable this definition after reviewing permissions.".into());continue;}
                if schedule.next_at.is_none_or(|at|at<=now){
                    let late=schedule.next_at.is_none_or(|at|now.saturating_sub(at)>milliseconds(schedule.interval));
                    if restarted ||late{skip(schedule,&mut store.occurrences,now,"Missed interval skipped; no catch-up run was started.")?;}
                    else{due.push(schedule.id.clone());}
                }
            }Ok(due)
        }).map_err(Failure::Unavailable)
    }
}
fn skip(
    schedule: &mut Schedule,
    history: &mut BTreeMap<String, Occurrence>,
    now: u64,
    reason: &str,
) -> Result<(), String> {
    let id = super::super::service::id().map_err(|e| e.message().to_string())?;
    history.insert(
        id.clone(),
        Occurrence {
            id: id.clone(),
            schedule: schedule.id.clone(),
            revision: schedule.revision,
            due_at: now,
            receipt: id,
            task: None,
            phase: Phase::Stopped,
            started_at: None,
            deadline: None,
            finished_at: Some(now),
            reason: Some(reason.into()),
        },
    );
    schedule.next_at = Some(now.saturating_add(milliseconds(schedule.interval)));
    schedule.error = Some(reason.into());
    schedule.updated_at = now;
    Ok(())
}
