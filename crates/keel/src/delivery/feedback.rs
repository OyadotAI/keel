//! Manual GitHub refresh retains remote identities and never resolves from partial reads.
use super::{Correction, Failure, Feedback, Resolution, Service};

impl Service {
    /// A complete refresh may close explicit resolved threads; errors preserve prior feedback.
    pub(crate) fn refresh_feedback(
        &self,
        task: &str,
        reader: &mut impl crate::github_read::Read,
    ) -> Result<Feedback, Failure> {
        let record = self.record(task)?;
        let url = self.feedback_target(task)?;
        let (repository, number) = pr_link(&url)?;
        let feedback = crate::github_read::feedback(reader, &repository, number, self.clock.now())
            .map_err(Failure::Unavailable)?;
        self.merge_feedback(task, record.revision, feedback.clone())?;
        Ok(feedback)
    }

    pub(super) fn merge_feedback(
        &self,
        task: &str,
        revision: u64,
        feedback: Feedback,
    ) -> Result<(), Failure> {
        self.change(task, revision, |record| {
            for check in &feedback.checks {
                let outcome = check.conclusion.as_deref().unwrap_or(&check.status);
                if ![
                    "failure",
                    "timed_out",
                    "cancelled",
                    "action_required",
                    "error",
                ]
                .contains(&outcome)
                {
                    continue;
                }
                let id = format!("github-check:{}:{}", feedback.head, check.id);
                merge(
                    &mut record.corrections,
                    &id,
                    "github_check",
                    format!(
                        "{}: {}",
                        check.name,
                        check.conclusion.as_deref().unwrap_or(&check.status)
                    ),
                    check.url.clone(),
                    (false, false),
                    &feedback,
                );
            }
            for thread in &feedback.threads {
                let body = thread
                    .comments
                    .iter()
                    .map(|c| {
                        format!(
                            "{}{}\n{}",
                            c.path,
                            c.line
                                .or(c.original_line)
                                .map(|line| format!(":{line}"))
                                .unwrap_or_default(),
                            c.body
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let id = format!("github-thread:{}", thread.id);
                merge(
                    &mut record.corrections,
                    &id,
                    "github_thread",
                    body,
                    thread.comments.first().map(|c| c.url.clone()),
                    (thread.resolved, thread.outdated),
                    &feedback,
                );
            }
            record.feedback = Some(feedback);
            Ok(())
        })
    }
}

fn merge(
    items: &mut Vec<Correction>,
    remote: &str,
    source: &str,
    body: String,
    url: Option<String>,
    state: (bool, bool),
    feedback: &Feedback,
) {
    let (resolved, outdated) = state;
    if let Some(item) = items
        .iter_mut()
        .find(|c| c.remote_id.as_deref() == Some(remote))
    {
        if item.body != body || item.remote_open != Some(!resolved) || item.outdated != outdated {
            item.revision = item.revision.saturating_add(1);
        }
        item.body = body;
        item.url = url;
        item.remote_open = Some(!resolved);
        item.outdated = outdated;
        item.updated_at = feedback.fetched_at;
        if resolved && feedback.complete && item.resolution != Resolution::HumanResolved {
            item.resolution = Resolution::RemoteResolved;
        }
        if !resolved && item.resolution == Resolution::RemoteResolved {
            item.resolution = Resolution::Unresolved;
        }
    } else if !resolved && !body.trim().is_empty() {
        items.push(Correction {
            id: remote.into(),
            revision: 0,
            source: source.into(),
            remote_id: Some(remote.into()),
            anchor: None,
            body,
            url,
            resolution: Resolution::Unresolved,
            remote_open: Some(true),
            outdated,
            created_at: feedback.fetched_at,
            updated_at: feedback.fetched_at,
            deliveries: vec![],
        });
    }
}

/// Only a numeric github.com PR can supply a remote feedback target.
pub(super) fn pr_link(url: &str) -> Result<(String, u64), Failure> {
    let parts = url
        .strip_prefix("https://github.com/")
        .unwrap_or_default()
        .split('/')
        .collect::<Vec<_>>();
    if parts.len() != 4 || parts[2] != "pull" || parts[0].is_empty() || parts[1].is_empty() {
        return Err(Failure::Invalid(
            "Saved pull request link is invalid.".into(),
        ));
    }
    let number = parts[3]
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| Failure::Invalid("Saved pull request number is invalid.".into()))?;
    Ok((format!("{}/{}", parts[0], parts[1]), number))
}
