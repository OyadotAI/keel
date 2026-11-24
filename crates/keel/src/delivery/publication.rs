//! Publication saves one editable form and one immutable intent before contacting GitHub.
use super::{
    CheckoutIdentity, Failure, PrForm, Provenance, Publication, PublicationIntent, Service,
};
use crate::{
    execution::{Phase, Receipt},
    tasks::Source,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The same saved form is used by Delivery, review and an explicit publication request.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FormInput {
    pub revision: u64,
    pub form: PrForm,
}
/// A user request freezes the observed owner and checkout rather than publishing a later HEAD.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublishInput {
    pub receipt: String,
    pub revision: u64,
    pub owner: Source,
    pub identity: CheckoutIdentity,
    pub form: PrForm,
}

impl Service {
    /// Seed factual source and verification once, preserving every later human edit.
    pub(crate) fn seed_form(&self, task: &str) -> Result<PrForm, Failure> {
        let view = self.task(task)?;
        let record = view.record;
        if record.form.seeded {
            return Ok(record.form);
        }
        let source = match &record.source {
            Provenance::Issue { snapshot } => format!("Source: {}\n\n", snapshot.url),
            Provenance::Recurring { name, revision, .. } => {
                format!("Recurring task: {name} (revision {revision})\n\n")
            }
            _ => String::new(),
        };
        let check = record.checks.last();
        let proof = check
            .map(|c| {
                format!(
                    "Project checks: {}{}\nCommand: {}\nChecked commit: {}\n",
                    c.status,
                    if view.checks_current {
                        " (current)"
                    } else {
                        " (historical or unconfirmed)"
                    },
                    c.command.as_deref().unwrap_or("unknown"),
                    c.before
                        .as_ref()
                        .map(|i| i.head.as_str())
                        .unwrap_or("unknown")
                )
            })
            .unwrap_or("Project checks: not recorded\n".into());
        let form = PrForm {
            title: record.title.clone(),
            body: format!(
                "{source}## Verification\n\n{proof}\nAdvisory review is separate from project checks.\nUnresolved corrections: {}\n",
                record
                    .corrections
                    .iter()
                    .filter(|c| c.resolution == super::Resolution::Unresolved)
                    .count()
            ),
            base: record.base.clone(),
            seeded: true,
            ..Default::default()
        };
        self.save_form(
            task,
            FormInput {
                revision: record.revision,
                form,
            },
        )
    }

    /// The daemon persists the shared draft so window remounts and provider handoffs cannot erase it.
    pub(crate) fn save_form(&self, task: &str, input: FormInput) -> Result<PrForm, Failure> {
        if input.form.title.len() > super::constants::TEXT_LIMIT
            || input.form.body.len() > super::constants::TEXT_LIMIT
        {
            return Err(Failure::Invalid(
                "Pull request title and body must each fit within 20 KB.".into(),
            ));
        }
        self.change(task, input.revision, |record| {
            record.form = input.form;
            record.form.seeded = true;
            Ok(record.form.clone())
        })
    }

    /// An operation lease serializes retries; uncertainty always reuses the frozen target.
    pub(crate) async fn publish(
        self: Arc<Self>,
        task: String,
        input: PublishInput,
    ) -> Result<Receipt, Failure> {
        let service = self.clone();
        let request = input.clone();
        let target = task.clone();
        let receipt = crate::serve::in_blocking(move || {
            service.execution.reserve(
                &request.receipt,
                "publication",
                serde_json::json!({"task":target,"request":request}),
                service.clock.now(),
            )
        })
        .await
        .map_err(Failure::from)?;
        if receipt.phase == Phase::Completed {
            return Ok(receipt);
        }
        let service = self.clone();
        let key = input.receipt.clone();
        let lease = crate::serve::in_blocking(move || {
            crate::permissions::try_lease(&format!(
                "delivery-publish-{}-{key}.lock",
                crate::permissions::key_of(&service.root)?
            ))
        })
        .await
        .map_err(Failure::Unavailable)?;
        let Some(_lease) = lease else {
            return Ok(receipt);
        };
        let intent: PublicationIntent = if let Some(result) = receipt.result {
            serde_json::from_value(result["intent"].clone()).map_err(|_| {
                Failure::Unavailable(
                    "Captured publication is unreadable; no new target was used.".into(),
                )
            })?
        } else {
            let service = self.clone();
            let frozen = input.clone();
            let target = task.clone();
            crate::serve::in_blocking(move || {
                if service.owner(&target)? != frozen.owner
                    || service.task(&target)?.identity.as_ref() != Some(&frozen.identity)
                {
                    return Err(Failure::Conflict(
                        "Task owner or checkout changed. Refresh before publishing.".into(),
                    ));
                }
                if service.record(&target)?.revision != frozen.revision {
                    return Err(Failure::Conflict(
                        "Task changed. Refresh before capturing publication.".into(),
                    ));
                }
                validate_form(&frozen.form)?;
                Ok(())
            })
            .await?;
            let intent = self
                .runtime
                .capture_publication(
                    task.clone(),
                    input.owner.clone(),
                    input.identity.clone(),
                    input.form.clone(),
                )
                .await
                .map_err(Failure::Conflict)?;
            let service = self.clone();
            let key = input.receipt.clone();
            let saved = intent.clone();
            crate::serve::in_blocking(move || {
                service.execution.change(|store| {
                    let receipt = store
                        .receipts
                        .get_mut(&key)
                        .ok_or("Publication receipt unavailable.")?;
                    receipt.task = Some(saved.task.clone());
                    receipt.result = Some(serde_json::json!({"intent":saved}));
                    receipt.phase = Phase::Preparing;
                    Ok(())
                })
            })
            .await
            .map_err(Failure::Unavailable)?;
            let service = self.clone();
            let target = task.clone();
            let frozen = input.clone();
            crate::serve::in_blocking(move || {
                service.save_form(
                    &target,
                    FormInput {
                        revision: frozen.revision,
                        form: frozen.form,
                    },
                )
            })
            .await?;
            intent
        };
        let result = self.runtime.publish(intent.clone()).await;
        let service = self.clone();
        let captured = intent.clone();
        let target = task.clone();
        let previous = crate::serve::in_blocking(move || {
            service.execution.read().map(|store| {
                store
                    .receipts
                    .values()
                    .filter(|receipt| receipt.task.as_deref() == Some(&target))
                    .filter_map(|receipt| {
                        receipt
                            .result
                            .clone()
                            .and_then(|value| serde_json::from_value::<Publication>(value).ok())
                    })
                    .filter(|publication| {
                        publication.url.is_some()
                            && publication.intent.repository == captured.repository
                            && publication.intent.source_repository == captured.source_repository
                            && publication.intent.push_identity == captured.push_identity
                            && publication.intent.branch == captured.branch
                            && publication.intent.base == captured.base
                    })
                    .max_by_key(|publication| publication.updated_at)
            })
        })
        .await
        .map_err(Failure::Unavailable)?;
        let publication = publication_result(
            input.receipt.clone(),
            intent,
            result,
            previous,
            self.clock.now(),
        );
        let service = self.clone();
        let saved = publication.clone();
        crate::serve::in_blocking(move || {
            service
                .execution
                .change(|store| {
                    let receipt = store
                        .receipts
                        .get_mut(&saved.receipt)
                        .ok_or("Publication receipt unavailable.")?;
                    receipt.phase = if saved.status == "confirmed" {
                        Phase::Completed
                    } else {
                        Phase::Unconfirmed
                    };
                    receipt.error = saved.error.clone();
                    receipt.result = Some(serde_json::to_value(&saved).map_err(|e| e.to_string())?);
                    receipt.updated_at = service.clock.now();
                    Ok(())
                })
                .map_err(Failure::Unavailable)?;
            let revision = service.record(&task)?.revision;
            service.change(&task, revision, |record| {
                record.publication = Some(saved);
                Ok(())
            })
        })
        .await?;
        crate::events::emit(
            "delivery.changed",
            publication.intent.owner.wt.as_deref(),
            serde_json::json!({"task":publication.intent.task}),
        );
        self.read_receipt(&input.receipt).await
    }
}

fn validate_form(form: &PrForm) -> Result<(), Failure> {
    if form.title.len() > super::constants::TEXT_LIMIT
        || form.body.len() > super::constants::TEXT_LIMIT
    {
        return Err(Failure::Invalid(
            "Pull request title and body must each fit within 20 KB.".into(),
        ));
    }
    Ok(())
}
fn publication_result(
    receipt: String,
    intent: PublicationIntent,
    result: Result<(String, String), String>,
    previous: Option<Publication>,
    at: u64,
) -> Publication {
    let (url, actual_head, error) = match result {
        Ok((url, head)) => {
            let error=(head!=intent.identity.head).then(||"The pull request exists at a different commit. Publication of the captured commit is unconfirmed; inspect the saved target before retrying.".into());
            (Some(url), Some(head), error)
        }
        Err(error) => (
            previous
                .as_ref()
                .and_then(|publication| publication.url.clone()),
            previous.and_then(|publication| publication.actual_head),
            Some(error),
        ),
    };
    let confirmed = error.is_none() && actual_head.as_ref() == Some(&intent.identity.head);
    Publication {
        receipt,
        intent,
        status: if confirmed {
            "confirmed"
        } else {
            "unconfirmed"
        }
        .into(),
        url,
        actual_head,
        error,
        updated_at: at,
    }
}
