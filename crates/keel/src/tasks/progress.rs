//! Lane-scoped progress reports provide shared memory without trusting agent-supplied ownership.
use super::{
    model::*,
    repository::{Repository, event},
    text, tracking,
};
use crate::serve::AppState;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
pub(crate) const TOOL: &str = "task_memory";
/// One tool reads the latest checkpoint or reports structured claims with optimistic revision checks.
pub(crate) fn schema() -> Value {
    json!({"name":TOOL,"description":"Read this lane's shared task memory and changelog. At meaningful milestones and before stopping, report decisions, completed work, remaining work, blockers and next action for the next coding agent. Read first, then report using the returned generation and revision. Reports are claims, not verified checks. Never include secrets, reasoning, system prompts or raw tool dumps.","inputSchema":{"type":"object","properties":{"notes":{"type":"object","properties":{"decisions":{"type":"string"},"completed":{"type":"string"},"remaining":{"type":"string"},"blockers":{"type":"string"},"next_action":{"type":"string"}},"additionalProperties":false},"revision":{"type":"integer"},"generation":{"type":"string"}},"additionalProperties":false}})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    notes: Option<Notes>,
    revision: Option<u64>,
    generation: Option<String>,
}
pub(crate) async fn call(
    state: Arc<AppState>,
    lane: Option<String>,
    args: Value,
) -> Result<String, String> {
    let lane = lane.ok_or("Task memory requires a registered Keel lane.")?;
    let report: Report = serde_json::from_value(args).map_err(|e| e.to_string())?;
    crate::serve::in_blocking(move||{
        Repository::new(&state.repo())?.change(|store| {
            let found=tracking::find(store,&lane).ok_or("This lane has no registered task yet.")?.clone();
            if found.owner.lane!=lane {return Err("This source handed work to another coding agent. Its progress reports can no longer update the task.".into());}
            let generation=format!("{}:{}",found.id,found.owner.lane);
            let task=store.tasks.get_mut(&found.id).unwrap();
            if let Some(notes)=report.notes {
                if report.revision!=Some(task.revision)||report.generation.as_deref()!=Some(&generation){return Err("Task memory changed. Read the latest revision before reporting progress.".into());}
                task.notes=text::notes(notes);task.incomplete=false;task.revision=task.revision.saturating_add(1);task.updated_at=crate::turns::now();
                let snapshot=task.clone();event(store,&snapshot,"Agent-reported progress",format!("Decisions: {}\nCompleted: {}\nRemaining: {}\nBlockers: {}",snapshot.notes.decisions,snapshot.notes.completed,snapshot.notes.remaining,snapshot.notes.blockers));
            }
            let task=store.tasks.get(&found.id).unwrap();
            memory_reply(store, task, &generation)
        })
    }).await
}

const REPLY_FIELD_CHARS: usize = 1024;
const REPLY_STEERING: usize = 4;
const REPLY_EVIDENCE: usize = 8;
const REPLY_EVENTS: usize = 4;
const REPLY_BYTES: usize = 64 * 1024;
fn short(value: &str) -> String {
    value.chars().take(REPLY_FIELD_CHARS).collect()
}
fn memory_reply(store: &Store, task: &Task, generation: &str) -> Result<String, String> {
    let notes = &task.notes;
    let summary = json!({
        "id":task.id,"owner":task.owner,"objective":short(&task.objective),
        "steering":task.steering.iter().rev().take(REPLY_STEERING).map(|s|short(s)).collect::<Vec<_>>(),
        "notes":{"decisions":short(&notes.decisions),"completed":short(&notes.completed),"remaining":short(&notes.remaining),"blockers":short(&notes.blockers),"next_action":short(&notes.next_action)},
        "evidence":task.evidence.iter().rev().take(REPLY_EVIDENCE).map(|s|short(s)).collect::<Vec<_>>(),"incomplete":task.incomplete,
    });
    let events = store.events.iter().rev().filter(|e|e.task==task.id).take(REPLY_EVENTS)
        .map(|e|json!({"sequence":e.sequence,"at":e.at,"provider":e.source.provider,"session":e.source.session,"kind":e.kind,"summary":short(&e.summary),"next_action":short(&e.next_action)})).collect::<Vec<_>>();
    let reply = serde_json::to_string(&json!({"generation":generation,"revision":task.revision,"task":summary,"changelog":events,"truncated":true,"full_notes":"Project .keel/tasks/state.json and .keel/CHANGELOG.md retain bounded history; this tool returns a compact checkpoint."})).map_err(|e|e.to_string())?;
    if reply.len() > REPLY_BYTES {
        return Err(
            "Task memory preview exceeds its display limit. Inspect the saved handoff in Context."
                .into(),
        );
    }
    Ok(reply)
}
