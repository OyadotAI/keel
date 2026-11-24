//! Prepared checkouts are recovered only when their branch, commit and project still match.
use super::{Failure, Service, launch::LaunchPlan};
impl Service {
    pub(super) fn prepare_checkout(&self, plan: &LaunchPlan) -> Result<Vec<String>, Failure> {
        if !self.runtime.available(&plan.owner.provider) {
            return Err(Failure::Invalid(
                "The selected coding agent is not installed. Install it, then retry this start."
                    .into(),
            ));
        }
        let wt = plan
            .owner
            .wt
            .as_deref()
            .ok_or_else(|| Failure::Invalid("An isolated checkout is required.".into()))?;
        let checkout = camino::Utf8Path::new(&plan.owner.checkout);
        if checkout != crate::worktree::path_of(&self.root, wt).map_err(Failure::Invalid)?
            || plan.task != crate::tasks::key_for(&plan.owner)
        {
            return Err(Failure::Conflict(
                "Saved launch identity does not match its original project checkout.".into(),
            ));
        }
        if checkout.exists() {
            let branch = crate::git::trimmed(checkout, &["symbolic-ref", "--short", "HEAD"])
                .map_err(Failure::Conflict)?;
            let head =
                crate::git::trimmed(checkout, &["rev-parse", "HEAD"]).map_err(Failure::Conflict)?;
            let common = crate::git::trimmed(
                checkout,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .map_err(Failure::Conflict)?;
            let expected = crate::git::trimmed(
                &self.root,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .map_err(Failure::Conflict)?;
            if branch != format!("keel/{wt}") || head != plan.sha || common != expected {
                return Err(Failure::Conflict("The prepared checkout changed or its name is already used. No second agent was started.".into()));
            }
        } else {
            crate::worktree::create_exact(&self.root, wt, &plan.base, &plan.sha)
                .map_err(Failure::Conflict)?;
        }
        let mut warnings = Vec::new();
        let task = match crate::tasks::register(&self.root, plan.owner.clone()) {
            Ok(task) => task,
            Err(error) => {
                let saved = crate::tasks::get(&self.root, &plan.task)
                    .map_err(|_| Failure::Conflict(error))?;
                if saved.owner != plan.owner {
                    return Err(Failure::Conflict(
                        "Task ownership changed during registration.".into(),
                    ));
                }
                warnings.push(
                    "Task saved; its shared Markdown changelog could not be refreshed.".into(),
                );
                saved.id
            }
        };
        if task != plan.task {
            return Err(Failure::Conflict(
                "Task registration returned another identity. Inspect the saved request.".into(),
            ));
        }
        if let Err(error) = crate::tasks::pin(&self.root, &task) {
            if !crate::tasks::get(&self.root, &task).is_ok_and(|task| task.pinned) {
                return Err(Failure::Unavailable(error));
            }
            if warnings.is_empty() {
                warnings.push(
                    "Task saved; its shared Markdown changelog could not be refreshed.".into(),
                );
            }
        }
        self.records
            .change(|store| {
                store.records.entry(task.clone()).or_insert_with(|| {
                    self.empty_record(
                        &task,
                        plan.title.clone(),
                        plan.owner.clone(),
                        plan.source.clone(),
                        (plan.base.clone(), plan.sha.clone(), format!("keel/{wt}")),
                    )
                });
                if let Some(preview) = &plan.preview {
                    store.previews.remove(preview);
                }
                Ok(())
            })
            .map_err(Failure::Unavailable)?;
        Ok(warnings)
    }
}
