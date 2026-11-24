//! Durable delivery uses the existing immutable publisher and checkout reservation.
use super::{PrQuery, github, repository, target};
use crate::{
    delivery::{CheckoutIdentity, PrForm, PublicationIntent},
    serve::{AppState, Held},
    tasks::Source,
};
use camino::Utf8Path;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Capture hosting and target before mutation; raw credential-bearing URLs never enter metadata.
pub(crate) fn capture(
    task: String,
    owner: Source,
    identity: CheckoutIdentity,
    form: PrForm,
) -> Result<PublicationIntent, String> {
    let repo = Utf8Path::new(&owner.checkout);
    if crate::delivery::capture(repo)? != identity {
        return Err("Checkout changed. Refresh before publishing.".into());
    }
    let branch = super::precheck(repo, &form.base)?;
    let hosting = repository::capture(&mut |program, args| github::command(repo, program, args))?;
    Ok(PublicationIntent {
        id: crate::delivery::operation_id().map_err(|e| e.message().to_string())?,
        task,
        owner,
        repository: hosting.base,
        source_repository: hosting.source,
        push_identity: format!("{:x}", Sha256::digest(hosting.origin.as_bytes())),
        branch,
        base: form.base.clone(),
        identity,
        form,
    })
}

/// A retry reconciles the frozen PR first; new pushes still require unchanged checkout and remote.
pub(crate) async fn publish(
    state: Arc<AppState>,
    intent: PublicationIntent,
) -> Result<(String, String), String> {
    let repo = camino::Utf8PathBuf::from(&intent.owner.checkout);
    let (s, r) = (state.clone(), repo.clone());
    let _held = crate::serve::in_blocking(move || {
        crate::tasks::guard(&s.repo(), &intent.owner.lane, &r)?;
        let key = format!("keel:pr:{r}");
        let token = s.reserve_checkout(&key, &r)?;
        Ok::<_, String>(Held::new(s, key, token))
    })
    .await?;
    crate::prereview::cancel_for_open(&repo).await?;
    crate::serve::in_blocking(move || {
        let hosting =
            repository::capture(&mut |program, args| github::command(&repo, program, args))?;
        if hosting.base != intent.repository
            || hosting.source != intent.source_repository
            || format!("{:x}", Sha256::digest(hosting.origin.as_bytes())) != intent.push_identity
        {
            return Err(
                "The publication repository changed. Inspect the captured request before retrying."
                    .into(),
            );
        }
        let target = target::Target {
            branch: intent.branch,
            sha: intent.identity.head,
            base: intent.base,
        };
        let form = intent.form;
        let query = PrQuery {
            title: form.title,
            body: form.body,
            draft: form.draft,
            base: target.base.clone(),
            head: target.sha.clone(),
            branch: target.branch.clone(),
        };
        let opened = github::publish(
            &repo,
            &target,
            &hosting,
            &query,
            &mut |program, args| github::command(&repo, program, args),
            &mut |_| {},
        )?;
        super::remember(&repo, &target.branch, &opened.url);
        Ok((opened.url, opened.head))
    })
    .await
}
