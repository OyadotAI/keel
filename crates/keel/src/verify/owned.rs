//! Manual checks use the ordinary checkout claim and persist their exact execution-time captures.
use super::{GateEvent, run_all};
use crate::{
    delivery::CheckEvidence,
    serve::{AppState, Held},
};
use camino::Utf8PathBuf;
use std::sync::Arc;

struct Claimed {
    _held: Held,
    state: Arc<AppState>,
    id: String,
    token: u64,
}
impl Drop for Claimed {
    fn drop(&mut self) {
        self.state.finish_check(&self.id, self.token);
    }
}

/// HTTP streams and durable task actions share one claimed checker, including cancellation and history.
pub(crate) async fn check_owned(
    state: Arc<AppState>,
    checkout: Utf8PathBuf,
    lane: Option<String>,
    id: Option<String>,
    sink: impl FnMut(GateEvent) + Send,
    cancelled: impl Fn() -> bool + Send,
) -> Result<CheckEvidence, String> {
    let root = state.repo();
    let setup_state = state.clone();
    let setup_checkout = checkout.clone();
    let setup_lane = lane.clone();
    let (held, token, key, mut evidence) = crate::serve::in_blocking(move || {
        crate::tasks::guard(
            &setup_state.repo(),
            setup_lane.as_deref().unwrap_or_default(),
            &setup_checkout,
        )?;
        let id = match id {
            Some(id) => id,
            None => crate::delivery::operation_id().map_err(|e| e.message().to_string())?,
        };
        let key = setup_lane
            .clone()
            .unwrap_or_else(|| format!("keel:manual-check:{id}"));
        let token = setup_state.claim(&key, &setup_checkout, true)?;
        let held = Held::new(setup_state.clone(), key.clone(), token);
        setup_state.finished(&key, token);
        let mut evidence = crate::delivery::begin_check(
            &setup_checkout,
            format!("manual:{}", setup_lane.as_deref().unwrap_or("project")),
            None,
        )
        .map_err(|e| e.message().to_string())?;
        evidence.id = id.clone();
        setup_state.register_check(&id, &key, token);
        let held = Claimed {
            _held: held,
            state: setup_state.clone(),
            id,
            token,
        };
        save(
            &setup_state,
            &setup_checkout,
            setup_lane.as_deref(),
            &mut evidence,
        )?;
        Ok::<_, String>((held, token, key, evidence))
    })
    .await?;
    let verdict = run_all(&checkout, sink, || {
        cancelled()
            || state.repo() != root
            || state.was_interrupted(&key, token)
            || state.checks_stopped(&key, token)
    })
    .await;
    let result_state = state.clone();
    let result_checkout = checkout.clone();
    let evidence = crate::serve::in_blocking(move || {
        crate::delivery::end_check(
            &result_checkout,
            &mut evidence,
            verdict.status,
            Some(verdict.command),
            verdict
                .problems
                .iter()
                .map(crate::turns::Problem::from)
                .collect(),
        );
        if let Err(error) = save(
            &result_state,
            &result_checkout,
            lane.as_deref(),
            &mut evidence,
        ) {
            evidence.warning = Some(error);
        }
        Ok::<_, String>(evidence)
    })
    .await?;
    drop(held);
    Ok(evidence)
}

fn save(
    state: &Arc<AppState>,
    checkout: &camino::Utf8Path,
    lane: Option<&str>,
    evidence: &mut CheckEvidence,
) -> Result<(), String> {
    let Some(lane) = lane else { return Ok(()) };
    let root = state.repo();
    if let Some(task) = crate::tasks::for_lane(&root, lane)? {
        let owner = crate::tasks::owner(&root, &task)?;
        if let Err(error) =
            crate::delivery::persist_check(&root, &owner.session, checkout, evidence)
        {
            evidence.warning = Some(error);
        }
    }
    crate::turns::emit_for_lane(
        state,
        lane,
        crate::turns::Fact::Gate {
            status: evidence.status.clone(),
            command: evidence.command.clone(),
            ms: evidence
                .completed_at
                .map(|at| at.saturating_sub(evidence.started_at)),
            problems: evidence.problems.clone(),
            evidence: Some(Box::new(evidence.clone())),
        },
    );
    Ok(())
}
