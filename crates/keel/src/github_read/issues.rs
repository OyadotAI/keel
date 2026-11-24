//! Issue snapshots require exact repository membership and reject pull requests from the Issues API.
use super::{
    Project, Read,
    transport::{clipped, text, valid_repository},
};
use crate::delivery::{Issue, IssuePreview};

/// Canonical issue parsing never accepts foreign hosts, credentials or ambiguous path components.
pub(crate) fn issue_url(value: &str) -> Result<(String, u64), String> {
    if value
        .trim()
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .split('/')
        .any(|part| part == "." || part == ".." || part.contains('%'))
    {
        return Err(invalid());
    }
    let url = reqwest::Url::parse(value.trim()).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(invalid());
    }
    let parts: Vec<_> = url
        .path()
        .trim_end_matches('/')
        .split('/')
        .skip(1)
        .collect();
    if parts.len() != 4 || parts[2] != "issues" {
        return Err(invalid());
    }
    let repository = format!("{}/{}", parts[0], parts[1]);
    let number = parts[3]
        .parse::<u64>()
        .ok()
        .filter(|n| *n != 0)
        .ok_or_else(invalid)?;
    if !valid_repository(&repository) {
        return Err(invalid());
    }
    Ok((repository, number))
}

fn invalid() -> String {
    "Enter a GitHub.com issue URL, such as https://github.com/owner/repo/issues/123.".into()
}

/// Start only sees a saved server snapshot, never a client-supplied title masquerading as GitHub data.
pub(crate) fn preview(
    reader: &mut impl Read,
    project: &Project,
    url: &str,
    id: String,
    now: u64,
) -> Result<IssuePreview, String> {
    let (repository, number) = issue_url(url)?;
    if !project.source.eq_ignore_ascii_case(&repository)
        && !project
            .parent
            .as_ref()
            .is_some_and(|p| p.eq_ignore_ascii_case(&repository))
    {
        return Err(format!(
            "This issue belongs to {repository}. Open that repository to work on it here."
        ));
    }
    let value = reader.rest(&format!("repos/{repository}/issues/{number}"))?;
    if value.get("pull_request").is_some() {
        return Err(
            "This link is a pull request. Paste an issue URL to start an issue task.".into(),
        );
    }
    if value["number"].as_u64() != Some(number) {
        return Err("GitHub returned another issue identity. Preview again.".into());
    }
    let source = text(&value, "html_url")?;
    let (canonical, actual_number) = issue_url(&source)?;
    if !canonical.eq_ignore_ascii_case(&repository) || actual_number != number {
        return Err(
            "GitHub changed the source repository. Preview its canonical issue URL again.".into(),
        );
    }
    let (body, truncated) = clipped(value["body"].as_str().unwrap_or_default());
    let issue = Issue {
        id: text(&value, "node_id")?,
        number,
        repository: canonical,
        url: source,
        title: text(&value, "title")?,
        body,
        state: text(&value, "state")?,
        updated_at: text(&value, "updated_at")?,
        fetched_at: now,
        truncated,
    };
    Ok(IssuePreview {
        id,
        issue,
        change_repository: project.source.clone(),
        parent: project.parent.clone(),
    })
}
