//! Explicit branch and commit identity keeps a retry from publishing a different checkout state.
use super::{PrQuery, precheck};
use camino::Utf8Path;

const OBJECT_ID_LENGTHS: [usize; 2] = [40, 64];

/// Complete object IDs are safe immutable refs, unlike option-like or ambiguous user strings.
pub(super) fn valid_object_id(value: &str) -> bool {
    OBJECT_ID_LENGTHS.contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Clone, Debug)]
pub(super) struct Target {
    pub branch: String,
    pub sha: String,
    pub base: String,
}

pub(super) fn capture(repo: &Utf8Path, query: &PrQuery) -> Result<Target, String> {
    let base = if query.base.trim().is_empty() {
        crate::prereview::remote_default(repo)
    } else {
        query.base.trim().to_string()
    };
    crate::git::run(repo, &["check-ref-format", "--branch", &base])?;
    if !query.head.is_empty() && !query.branch.is_empty() {
        crate::git::run(repo, &["check-ref-format", "--branch", &query.branch])?;
        if !valid_object_id(&query.head) {
            return Err("The captured pull request commit is invalid. Refresh its target.".into());
        }
        return Ok(Target {
            branch: query.branch.clone(),
            sha: query.head.clone(),
            base,
        });
    }
    let branch = precheck(repo, &base)?;
    let sha = crate::git::trimmed(repo, &["rev-parse", "HEAD"])?;
    if (!query.head.is_empty() && query.head != sha)
        || (!query.branch.is_empty() && query.branch != branch)
    {
        return Err("The branch changed. Refresh the pull request target before opening.".into());
    }
    Ok(Target { branch, sha, base })
}

pub(super) fn unchanged(repo: &Utf8Path, target: &Target) -> Result<(), String> {
    if crate::git::trimmed(repo, &["rev-parse", "HEAD"])? != target.sha
        || crate::git::trimmed(repo, &["branch", "--show-current"])? != target.branch
    {
        return Err("The branch changed while opening the pull request. Refresh its target before retrying.".into());
    }
    Ok(())
}

/// Existing links can be recovered after checkout changes; new publication still needs prechecks.
pub(super) fn publishable(repo: &Utf8Path, target: &Target) -> Result<(), String> {
    unchanged(repo, target)?;
    precheck(repo, &target.base)?;
    Ok(())
}
