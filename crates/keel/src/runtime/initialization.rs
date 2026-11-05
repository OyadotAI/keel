//! Provider-owned launch requests avoid rebuilding context that a live session discards.
use super::*;
use camino::Utf8Path;

fn prepare(state: &AppState, q: &ChatQuery, cwd: &Utf8Path) -> Result<Value, String> {
    let lane = q.lane.as_deref().ok_or("A lane is required")?;
    let provider = q.provider.as_deref().unwrap_or("claude");
    crate::browser::prepare(
        &state.repo(),
        lane,
        provider,
        q.session.as_deref().unwrap_or(lane),
        q.session.is_some(),
    )?;
    let system = format!(
        "{}\n{}",
        crate::agent::system_prompt(cwd),
        crate::browser::instructions(Some(lane))
    );
    let (system, args) = if provider == "codex" {
        (
            crate::guidance::codex_system(cwd, &system),
            crate::askmcp::codex_args(cwd, state.port(), Some(lane))?,
        )
    } else {
        (system, Vec::new())
    };
    let mcp: Value = serde_json::from_str(&crate::askmcp::config(state.port(), Some(lane)))
        .map_err(|e| e.to_string())?;
    Ok(json!({"system":system,"codexArgs":args,"mcp":mcp}))
}

/// Resolve a cold-provider request from the held turn's paths and permissions, never helper input.
pub(super) async fn respond(
    state: &Arc<AppState>,
    runtime: &Runtime,
    q: &ChatQuery,
    cwd: &camino::Utf8PathBuf,
    token: u64,
    data: &Value,
) -> Result<bool, String> {
    let id = data["id"]
        .as_str()
        .filter(|s| s.len() <= 128)
        .ok_or("Invalid initialization request")?;
    let (owner, query, checkout) = (state.clone(), q.clone(), cwd.clone());
    let launch = tokio::task::spawn_blocking(move || prepare(&owner, &query, &checkout))
        .await
        .map_err(|e| e.to_string())
        .and_then(|v| v);
    let lane = q.lane.as_deref().ok_or("A lane is required")?;
    let generation = crate::browser::binding_id(lane);
    {
        let mut owned = runtime.browser_generation.locked();
        if !runtime.alive.load(Ordering::Acquire) {
            crate::browser::release(lane, generation.as_deref());
            return Err("The runtime closed during provider initialization".into());
        }
        *owned = generation;
    }
    let launch = if state.was_interrupted(lane, token) {
        Err("The turn was cancelled during provider initialization".into())
    } else {
        launch
    };
    let supplied = launch.as_ref().is_ok_and(|v| {
        v["system"]
            .as_str()
            .is_some_and(|s| s.contains("Keel collaboration guidance"))
    });
    let response = match launch {
        Ok(launch) => json!({"method":"initialized","id":id,"launch":launch}),
        Err(error) => json!({"method":"initialized","id":id,"error":error}),
    };
    runtime.send(response).await?;
    Ok(supplied)
}
