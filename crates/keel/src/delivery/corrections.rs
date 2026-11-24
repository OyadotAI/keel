//! Corrections retain their original anchors and require an explicit resolution decision.
use super::{Anchor, Correction, Failure, Resolution, Service, constants::TEXT_LIMIT, service::id};
use serde::{Deserialize, Serialize};

/// Optimistic revisions preserve a person's draft when another window edits the same task.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CorrectionInput {
    pub revision: u64,
    pub body: String,
    pub anchor: Option<Anchor>,
}
/// Editing and resolving are independent from sending a correction to an agent.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CorrectionEdit {
    pub revision: u64,
    pub body: Option<String>,
    pub resolved: Option<bool>,
}

impl Service {
    /// Save the selected original range even if later checkout edits make it historical.
    pub(crate) fn add_correction(
        &self,
        task: &str,
        input: CorrectionInput,
    ) -> Result<Correction, Failure> {
        validate_body(&input.body)?;
        if let Some(anchor) = &input.anchor {
            validate_anchor(anchor)?;
        }
        let at = self.clock.now();
        let identity = self.task(task)?.identity;
        let correction = Correction {
            id: id()?,
            revision: 0,
            source: "local".into(),
            remote_id: None,
            outdated: input
                .anchor
                .as_ref()
                .is_some_and(|a| Some(&a.identity) != identity.as_ref()),
            anchor: input.anchor,
            body: input.body,
            url: None,
            resolution: Resolution::Unresolved,
            remote_open: None,
            created_at: at,
            updated_at: at,
            deliveries: vec![],
        };
        self.change(task, input.revision, |record| {
            record.corrections.push(correction.clone());
            Ok(correction)
        })
    }

    /// Explicit human resolution never erases source text or the receipts that delivered it.
    pub(crate) fn edit_correction(
        &self,
        task: &str,
        id: &str,
        input: CorrectionEdit,
    ) -> Result<Correction, Failure> {
        if let Some(body) = &input.body {
            validate_body(body)?;
        }
        let at = self.clock.now();
        self.change(task, input.revision, |record| {
            let correction = record
                .corrections
                .iter_mut()
                .find(|c| c.id == id)
                .ok_or("Correction is unavailable.")?;
            if let Some(body) = input.body {
                correction.body = body;
            }
            if let Some(resolved) = input.resolved {
                correction.resolution = if resolved {
                    Resolution::HumanResolved
                } else {
                    Resolution::Unresolved
                };
            }
            correction.revision = correction
                .revision
                .checked_add(1)
                .ok_or("Correction revision is exhausted.")?;
            correction.updated_at = at;
            Ok(correction.clone())
        })
    }
}

fn validate_body(body: &str) -> Result<(), Failure> {
    if body.trim().is_empty() || body.len() > TEXT_LIMIT {
        return Err(Failure::Invalid("Enter a correction within 20 KB.".into()));
    }
    Ok(())
}
fn validate_anchor(anchor: &Anchor) -> Result<(), Failure> {
    let path = std::path::Path::new(&anchor.path);
    let valid = !anchor.path.is_empty()
        && !path.is_absolute()
        && !anchor.path.contains('\\')
        && path
            .components()
            .all(|p| matches!(p, std::path::Component::Normal(_)));
    let range = match (anchor.first, anchor.last) {
        (Some(first), Some(last)) => first > 0 && last >= first,
        (None, None) => true,
        _ => false,
    };
    if !valid
        || !range
        || !["old", "new", "file"].contains(&anchor.side.as_str())
        || anchor.quote.len() > TEXT_LIMIT
        || !crate::github_read::valid_sha(&anchor.identity.head)
        || !crate::github_read::valid_sha(&anchor.identity.tree)
    {
        return Err(Failure::Invalid(
            "Choose a valid file and original diff range.".into(),
        ));
    }
    Ok(())
}
