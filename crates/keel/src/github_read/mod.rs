//! Bounded read-only GitHub evidence never mutates issues, threads or publication targets.
mod checks;
mod constants;
mod issues;
#[cfg(test)]
mod tests;
mod threads;
mod transport;
pub(crate) use issues::preview;
pub(crate) use transport::{Gh, Project, Read};

/// Capture and recheck one PR head around independent check/thread reads; movement remains unconfirmed.
pub(crate) fn feedback(
    reader: &mut impl Read,
    repository: &str,
    number: u64,
    now: u64,
) -> Result<crate::delivery::Feedback, String> {
    if !transport::valid_repository(repository) || number == 0 {
        return Err("Invalid associated pull request.".into());
    }
    let path = format!("repos/{repository}/pulls/{number}");
    let first = reader.rest(&path)?;
    let head = transport::text(&first["head"], "sha")?;
    if !valid_sha(&head) {
        return Err("GitHub omitted the pull request commit identity.".into());
    }
    let (checks, mut errors) = checks::read(reader, repository, &head);
    let (threads, thread_errors) = threads::read(reader, repository, number, &head);
    errors.extend(thread_errors);
    match reader.rest(&path) {
        Ok(last) if last["head"]["sha"].as_str() == Some(&head) => {}
        _ => errors.push(
            "Pull request head could not be reconfirmed. Refresh before sending feedback.".into(),
        ),
    }
    Ok(crate::delivery::Feedback {
        head,
        fetched_at: now,
        complete: errors.is_empty(),
        checks,
        threads,
        errors,
    })
}

/// Full Git object identities are required; an abbreviated SHA cannot establish evidence currency.
pub(crate) fn valid_sha(value: &str) -> bool {
    [40, 64].contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
