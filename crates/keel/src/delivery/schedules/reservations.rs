//! One durable slot serializes occurrence reservations and stop ownership.
use super::*;
use sha2::Digest;
impl Service {
    pub(super) fn reserve_occurrence(
        &self,
        id: &str,
        revision: Option<u64>,
        receipt: &str,
        automatic: bool,
    ) -> Result<Occurrence, Failure> {
        if !crate::execution::valid_id(receipt) {
            return Err(Failure::Invalid("Invalid recurring request ID.".into()));
        }
        let at = self.clock.now();
        self.execution.change(|store|{
            if let Some(old)=store.occurrences.values().find(|o|o.receipt==receipt){return if old.schedule==id{Ok(old.clone())}else{Err("This request belongs to another definition.".into())};}
            if store.receipts.contains_key(receipt){return Err("This request ID was already used for another action.".into());}
            let schedule=store.schedules.get_mut(id).ok_or("Definition unavailable.")?;
            if revision.is_some_and(|revision|revision!=schedule.revision){return Err("Definition changed. Refresh before starting it.".into());}
            if automatic && (!schedule.enabled ||crate::permissions::trust_stamp(&self.root).as_deref()!=Some(&schedule.trust)){return Err("Recurring authorization changed. Explicitly enable this definition again.".into());}
            let overlap=store.active.is_some();
            let occurrence=Occurrence{id:receipt.into(),schedule:id.into(),revision:schedule.revision,due_at:at,receipt:receipt.into(),task:None,phase:if overlap{Phase::Stopped}else{Phase::Reserved},started_at:None,deadline:None,finished_at:overlap.then_some(at),reason:overlap.then(||"Skipped: another scheduled occurrence owns the project slot.".into())};
            if automatic{schedule.next_at=Some(at.saturating_add(milliseconds(schedule.interval)));schedule.error=None;}
            if !overlap{
                let input=serde_json::json!({"schedule":schedule,"occurrence":receipt,"automatic":automatic});
                let fingerprint=format!("{:x}",sha2::Sha256::digest(serde_json::to_vec(&input).map_err(|e|e.to_string())?));
                store.receipts.insert(receipt.into(),crate::execution::Receipt{id:receipt.into(),kind:"scheduled".into(),task:None,fingerprint,input,phase:Phase::Reserved,result:None,error:None,created_at:at,updated_at:at});store.active=Some(receipt.into());
            }
            store.occurrences.insert(receipt.into(),occurrence.clone());Ok(occurrence)
        }).map_err(Failure::Conflict)
    }

    pub(super) async fn stop_occurrence(
        self: &Arc<Self>,
        schedule: &str,
        confirmed: bool,
    ) -> Result<serde_json::Value, Failure> {
        let service = self.clone();
        let requested = schedule.to_string();
        let occurrence = crate::serve::in_blocking(move || {
            let store = service.execution.read().map_err(Failure::Unavailable)?;
            store
                .active
                .and_then(|id| store.occurrences.get(&id).cloned())
                .filter(|o| o.schedule == requested)
                .ok_or_else(|| {
                    Failure::Conflict("This definition has no active occurrence.".into())
                })
        })
        .await?;
        let service = self.clone();
        let id = occurrence.id.clone();
        let lease = crate::serve::in_blocking(move || {
            crate::permissions::try_lease(&format!(
                "occurrence-{}-{id}.lock",
                crate::permissions::key_of(&service.root)?
            ))
        })
        .await
        .map_err(Failure::Unavailable)?;
        let _lease = lease.ok_or_else(|| {
            Failure::Conflict(
                "This occurrence is being prepared. Retry Stop using the same occurrence.".into(),
            )
        })?;
        let owner = self.occurrence_owner(&occurrence).await?;
        let never_started = owner.is_none()
            && self
                .read_receipt(&occurrence.receipt)
                .await?
                .result
                .as_ref()
                .is_none_or(|plan| plan["dispatched"] == false);
        let confirmed = confirmed || never_started;
        let stopped = if let Some(owner) = owner {
            if confirmed {
                if self.runtime.pending(&owner)
                    || self
                        .runtime
                        .busy(&owner)
                        .await
                        .map_err(Failure::Unavailable)?
                {
                    return Err(Failure::Conflict("The task still has execution or a pending request. Stop or answer it first.".into()));
                }
                true
            } else {
                self.runtime
                    .stop(owner)
                    .await
                    .map_err(Failure::Unavailable)?
            }
        } else {
            true
        };
        let service = self.clone();
        let id = occurrence.id.clone();
        crate::serve::in_blocking(move||service.execution.change(|store|{
            let occurrence=store.occurrences.get_mut(&id).ok_or("Occurrence unavailable.")?;
            occurrence.phase=if confirmed{Phase::Stopped}else{Phase::Stopping};occurrence.reason=Some(if confirmed{"Stopped state confirmed by the person."}else if stopped{"Stop requested; waiting for execution and checks to become idle."}else{"Stop is unconfirmed. Inspect the original task before confirming it stopped."}.into());
            if confirmed{occurrence.finished_at=Some(service.clock.now());if store.active.as_deref()==Some(&id){store.active=None;}}
            Ok(())
        })).await.map_err(Failure::Unavailable)?;
        changed();
        Ok(serde_json::json!({"stopping":!confirmed,"confirmed":confirmed}))
    }
}
