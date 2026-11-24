//! Queued feedback uses an operation lease and current ownership checks before acceptance.
use super::*;
impl Service {
    /// Queue controllers also run after reconnect; only the private operation lease may send.
    pub(crate) async fn dispatch_batch(self: &Arc<Self>, id: &str) -> Result<Receipt, Failure> {
        let service = self.clone();
        let key = id.to_string();
        let lease = crate::serve::in_blocking(move || {
            crate::permissions::try_lease(&format!(
                "delivery-batch-{}-{key}.lock",
                crate::permissions::key_of(&service.root)?
            ))
        })
        .await
        .map_err(Failure::Unavailable)?;
        let Some(_lease) = lease else {
            return self.read_receipt(id).await;
        };
        let receipt = self.read_receipt(id).await?;
        if receipt.phase != Phase::Queued {
            return self.batch_status(id).await;
        }
        let mut plan: BatchPlan = serde_json::from_value(
            receipt
                .result
                .clone()
                .ok_or_else(|| Failure::Unavailable("Saved batch is incomplete.".into()))?,
        )
        .map_err(|_| Failure::Unavailable("Saved batch is unreadable.".into()))?;
        let service = self.clone();
        let frozen = plan.clone();
        let valid = crate::serve::in_blocking(move || service.validate_queued(&frozen)).await;
        if let Err(error) = valid {
            self.save_phase(
                id,
                Phase::FailedBeforeAcceptance,
                Some(error.message().into()),
            )
            .await?;
            return self.read_receipt(id).await;
        }
        if self.runtime.pending(&plan.owner)
            || self
                .runtime
                .busy(&plan.owner)
                .await
                .map_err(Failure::Unavailable)?
        {
            return Ok(receipt);
        }
        if let Some(head) = &plan.remote_head {
            let service = self.clone();
            let task = plan.task.clone();
            let target = crate::serve::in_blocking(move || service.feedback_target(&task)).await?;
            if plan.remote_target.as_ref() != Some(&target) {
                self.save_phase(id,Phase::FailedBeforeAcceptance,Some("The captured pull request association changed. Refresh feedback and send a new batch.".into())).await?;
                return self.read_receipt(id).await;
            }
            let (repository, number) = super::super::feedback::pr_link(&target)?;
            if !self
                .runtime
                .feedback_head(repository, number)
                .await
                .is_ok_and(|observed| &observed == head)
            {
                self.save_phase(id,Phase::FailedBeforeAcceptance,Some("Pull request head changed or could not be confirmed. Refresh feedback before sending a new batch.".into())).await?;
                return self.read_receipt(id).await;
            }
        }
        let service = self.clone();
        let frozen = plan.clone();
        let acceptance = crate::serve::in_blocking(move || {
            let lease = crate::permissions::try_acceptance_lease(&service.root)
                .map_err(Failure::Unavailable)?;
            if lease.is_some() {
                service.validate_queued(&frozen)?;
            }
            Ok::<_, Failure>(lease)
        })
        .await;
        let _acceptance = match acceptance {
            Ok(Some(lease)) => lease,
            Ok(None) => return Ok(receipt),
            Err(error) => {
                self.save_phase(
                    id,
                    Phase::FailedBeforeAcceptance,
                    Some(error.message().into()),
                )
                .await?;
                return self.read_receipt(id).await;
            }
        };
        if self.runtime.pending(&plan.owner)
            || self
                .runtime
                .busy(&plan.owner)
                .await
                .map_err(Failure::Unavailable)?
        {
            return Ok(receipt);
        }
        plan.dispatched = true;
        let service = self.clone();
        let key = id.to_string();
        let frozen = plan.clone();
        crate::serve::in_blocking(move || {
            service.execution.change(|store| {
                let receipt = store
                    .receipts
                    .get_mut(&key)
                    .ok_or("Batch receipt unavailable.")?;
                receipt.result = Some(serde_json::to_value(frozen).map_err(|e| e.to_string())?);
                receipt.phase = Phase::Preparing;
                Ok(())
            })
        })
        .await
        .map_err(Failure::Unavailable)?;
        let result = self
            .runtime
            .submit(id.into(), query(&plan.owner, prompt(&plan), true))
            .await;
        let status = self
            .runtime
            .receipt(plan.owner.lane.clone(), id.into())
            .await;
        drop(_acceptance);
        let accepted = result.is_ok() || status.is_ok_and(|s| s.accepted);
        self.save_phase(
            id,
            if accepted {
                Phase::Accepted
            } else {
                Phase::Unconfirmed
            },
            result.err(),
        )
        .await?;
        if accepted {
            let service = self.clone();
            let key = id.to_string();
            let frozen = plan.clone();
            crate::serve::in_blocking(move || {
                let revision = service.record(&frozen.task)?.revision;
                service.change(&frozen.task, revision, |record| {
                    for correction in &mut record.corrections {
                        if frozen.corrections.iter().any(|c| c.id == correction.id)
                            && !correction.deliveries.contains(&key)
                        {
                            correction.deliveries.push(key.clone());
                        }
                    }
                    Ok(())
                })
            })
            .await?;
        }
        crate::events::emit(
            "delivery.changed",
            plan.owner.wt.as_deref(),
            serde_json::json!({"task":plan.task}),
        );
        self.read_receipt(id).await
    }
    fn validate_queued(&self, plan: &BatchPlan) -> Result<(), Failure> {
        if self.owner(&plan.task)? != plan.owner {
            return Err(Failure::Conflict("Task ownership changed. This batch was not sent; send a new batch to its current owner.".into()));
        }
        if crate::permissions::trust_stamp(&self.root) != plan.trust {
            return Err(Failure::Conflict(
                "Project trust changed. Send a new batch after reviewing its permissions.".into(),
            ));
        }
        if plan.remote_head.is_some()
            && plan.remote_target.as_ref() != Some(&self.feedback_target(&plan.task)?)
        {
            return Err(Failure::Conflict(
                "The captured pull request association changed. Send a new batch.".into(),
            ));
        }
        let record = self.record(&plan.task)?;
        if plan.corrections.iter().any(|selected| {
            !record.corrections.iter().any(|c| {
                c.id == selected.id
                    && c.revision == selected.revision
                    && c.resolution == Resolution::Unresolved
            })
        }) {
            return Err(Failure::Conflict(
                "Queued corrections changed or were resolved. Select a new batch.".into(),
            ));
        }
        validate_remote(&record, &plan.corrections, plan.remote_head.as_deref())
    }
}
