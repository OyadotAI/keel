//! Short-lived grid reads share parent authorization and never consume one SSE stream per child.
use super::{ID_LIMIT, watch};
use crate::serve::{Checkout, in_blocking};
use axum::{Json, http::StatusCode};
use keel_workspace::{
    Follows,
    subagents::{Availability, Detail, OUTPUT_BATCH_LIMIT},
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

const RESPONSE_BYTES: usize = 1024 * 1024;
const ITEM_BUDGET: usize = RESPONSE_BYTES / OUTPUT_BATCH_LIMIT / 2;
/// The bounded ID list cannot consume an unrestricted HTTP request body.
pub(crate) const REQUEST_BYTES: usize = 4096;

/// Only invocation IDs within the already authorized parent can select live output.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    parent: String,
    agent: Option<String>,
    invocations: Vec<String>,
}
fn validate(request: &Request) -> Result<Follows, String> {
    let ids = &request.invocations;
    if request.parent.is_empty()
        || request.parent.len() > ID_LIMIT
        || ids.is_empty()
        || ids.len() > OUTPUT_BATCH_LIMIT
        || ids.iter().any(|id| id.is_empty() || id.len() > ID_LIMIT)
        || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err("Invalid subagent output batch.".into());
    }
    Follows::from_query(request.agent.as_deref()).map_err(str::to_owned)
}
fn bounded(mut detail: Detail) -> Detail {
    let mut remaining = ITEM_BUDGET;
    detail.items.reverse();
    detail.items.retain(|item| {
        let bytes = serde_json::to_vec(item).map_or(ITEM_BUDGET + 1, |v| v.len());
        if bytes > remaining {
            detail.truncated = true;
            return false;
        }
        remaining -= bytes;
        true
    });
    detail.items.reverse();
    detail
}
/// One poll resolves all selected invocations from a single verified parent lifecycle scan.
pub(crate) async fn batch(
    Checkout(repo): Checkout,
    Json(request): Json<Request>,
) -> Result<Json<BTreeMap<String, Detail>>, (StatusCode, String)> {
    let agent = validate(&request).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    in_blocking(move || {
        let parent = agent
            .locate(&repo, &request.parent)
            .ok_or("Parent conversation is unavailable.")?;
        let reader = watch(parent, &repo, &request.parent);
        let mut reader = reader.lock().unwrap_or_else(|e| e.into_inner());
        let mut details = reader.details(&request.invocations);
        for id in request.invocations {
            details.entry(id).or_insert(Detail {
                items: vec![],
                availability: Availability::Unavailable,
                truncated: false,
            });
        }
        let details = details
            .into_iter()
            .map(|(id, detail)| (id, bounded(detail)))
            .collect::<BTreeMap<_, _>>();
        if serde_json::to_vec(&details)
            .map_err(|e| e.to_string())?
            .len()
            > RESPONSE_BYTES
        {
            return Err("Subagent output batch exceeds its limit.".into());
        }
        Ok(Json(details))
    })
    .await
    .map_err(|e| (StatusCode::NOT_FOUND, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batches_reject_duplicates_and_excessive_or_foreign_selectors() {
        let request = |ids| Request {
            parent: "parent".into(),
            agent: Some("codex".into()),
            invocations: ids,
        };
        assert!(validate(&request(vec!["one".into(), "two".into()])).is_ok());
        assert!(validate(&request(vec!["one".into(), "one".into()])).is_err());
        assert!(
            validate(&request(
                (0..=OUTPUT_BATCH_LIMIT).map(|n| n.to_string()).collect()
            ))
            .is_err()
        );
        let mut foreign = request(vec!["one".into()]);
        foreign.agent = Some("foreign".into());
        assert!(validate(&foreign).is_err());
    }
    #[test]
    fn escaping_and_large_output_cannot_exceed_the_aggregate_budget() {
        let detail = Detail {
            items: vec![keel_workspace::subagents::OutputLine {
                kind: "text".into(),
                text: "\u{001b}".repeat(128 * 1024),
            }],
            availability: Availability::Available,
            truncated: false,
        };
        let detail = bounded(detail);
        assert!(detail.truncated);
        assert!(serde_json::to_vec(&detail).unwrap().len() < ITEM_BUDGET);
    }
}
