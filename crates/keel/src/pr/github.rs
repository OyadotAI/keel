//! Host operations reconcile first and use exact refs, ordinary pushes, and bounded commands.
use super::{
    PrQuery,
    repository::Repository,
    target::{self, Target},
};
use camino::Utf8Path;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct Existing {
    pub url: String,
    #[serde(rename = "headRepository")]
    pub source: Source,
    #[serde(rename = "headRepositoryOwner")]
    pub owner: Owner,
    #[serde(rename = "headRefName")]
    pub branch: String,
    #[serde(rename = "baseRefName")]
    pub base: String,
    #[serde(rename = "headRefOid")]
    pub head: String,
}
#[derive(Debug, Deserialize)]
pub(super) struct Source {
    #[serde(rename = "nameWithOwner")]
    pub name: String,
}
#[derive(Debug, Deserialize)]
pub(super) struct Owner {
    pub login: String,
}
pub(super) struct Opened {
    pub url: String,
    pub existing: bool,
    pub head: String,
}
const COMMAND_WAIT: std::time::Duration = std::time::Duration::from_secs(60);
/// A push runs the repository's `pre-push` hook — often the whole test suite — and a first push of
/// a large branch on a slow uplink takes minutes. Sixty seconds killed both every time, so the pull
/// request could never open; this is still a ceiling, so the wait still ends.
const PUSH_WAIT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

pub(super) fn command(repo: &Utf8Path, program: &str, args: &[String]) -> Result<String, String> {
    if !["git", "gh"].contains(&program) {
        return Err("Unknown pull request command.".into());
    }
    let mut command = if program == "git" {
        crate::git::tokio_command(repo).into_std()
    } else {
        let mut c = std::process::Command::new(crate::permissions::program("gh"));
        c.current_dir(repo);
        c
    };
    command
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    let wait = if program == "git" && args.first().is_some_and(|a| a == "push") {
        PUSH_WAIT
    } else {
        COMMAND_WAIT
    };
    let output = crate::git::output_within(command, wait)
        .map_err(|e| format!("Could not run {program}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

fn lookup(
    target: &Target,
    repository: &Repository,
    run: &mut impl FnMut(&str, &[String]) -> Result<String, String>,
) -> Result<Option<Opened>, String> {
    let args = [
        "pr",
        "list",
        "--repo",
        &repository.base,
        "--state",
        "open",
        "--head",
        &target.branch,
        "--base",
        &target.base,
        "--json",
        "url,headRepository,headRepositoryOwner,headRefName,baseRefName,headRefOid",
        "--limit",
        "100",
    ]
    .map(str::to_string);
    let text = run("gh", &args)
        .map_err(|e| format!("Could not check for an existing pull request. {e}"))?;
    let found: Vec<Existing> = serde_json::from_str(&text).map_err(|_| "GitHub returned incomplete pull request identities. Check the connection before retrying.")?;
    let found: Vec<_> = found
        .into_iter()
        .filter(|p| {
            p.source.name.eq_ignore_ascii_case(&repository.source)
                && repository
                    .source
                    .split('/')
                    .next()
                    .is_some_and(|owner| owner.eq_ignore_ascii_case(&p.owner.login))
                && p.branch == target.branch
                && p.base == target.base
        })
        .collect();
    if found.len() > 1 {
        return Err("Several open pull requests match this branch and base. Check them on GitHub before retrying.".into());
    }
    found
        .into_iter()
        .next()
        .map(|p| {
            (valid_link(&p.url)
                && p.url
                    .strip_prefix("https://github.com/")
                    .and_then(|path| path.split_once("/pull/"))
                    .is_some_and(|(repo, _)| repo.eq_ignore_ascii_case(&repository.base))
                && target::valid_object_id(&p.head))
            .then_some(Opened {
                url: p.url,
                existing: true,
                head: p.head,
            })
            .ok_or("GitHub did not return a valid pull request link.".into())
        })
        .transpose()
}

fn valid_link(url: &str) -> bool {
    let Some(path) = url.strip_prefix("https://github.com/") else {
        return false;
    };
    let parts: Vec<_> = path.split('/').collect();
    parts.len() == 4
        && !parts[0].is_empty()
        && !parts[1].is_empty()
        && parts[2] == "pull"
        && parts[3].parse::<u64>().is_ok()
}

pub(super) fn publish(
    repo: &Utf8Path,
    target: &Target,
    repository: &Repository,
    query: &PrQuery,
    run: &mut impl FnMut(&str, &[String]) -> Result<String, String>,
    say: &mut impl FnMut(&str),
) -> Result<Opened, String> {
    say("Checking for an existing pull request…");
    if let Some(existing) = lookup(target, repository, run)? {
        return Ok(existing);
    }
    target::publishable(repo, target)?;
    let origin = run(
        "git",
        &["remote", "get-url", "--push", "--all", "origin"].map(str::to_string),
    )?;
    if origin != repository.origin {
        return Err("The push repository changed. Refresh the pull request target.".into());
    }
    say("Pushing the captured commit…");
    run(
        "git",
        &[
            "push".into(),
            "origin".into(),
            format!("{}:refs/heads/{}", target.sha, target.branch),
        ],
    )?;
    target::unchanged(repo, target)?;
    // Tracking config names the intended branch even if an external checkout moves later.
    let _ = run(
        "git",
        &[
            "branch".into(),
            format!("--set-upstream-to=origin/{}", target.branch),
            "--".into(),
            target.branch.clone(),
        ],
    );
    if let Some(existing) = lookup(target, repository, run)? {
        return Ok(existing);
    }
    say("Creating the pull request…");
    let mut args = vec![
        "pr".into(),
        "create".into(),
        "--repo".into(),
        repository.base.clone(),
        "--head".into(),
        repository.head(&target.branch),
        "--base".into(),
        target.base.clone(),
    ];
    if query.title.trim().is_empty() {
        args.push("--fill".into());
    } else {
        args.extend([
            "--title".into(),
            query.title.clone(),
            "--body".into(),
            query.body.clone(),
        ]);
    }
    if query.draft {
        args.push("--draft".into());
    }
    match run("gh", &args) {
        Ok(text) => {
            let mut confirmed = lookup(target, repository, run)?.ok_or(
                "GitHub did not confirm a pull request link. Check its status before retrying."
                    .to_string(),
            )?;
            confirmed.existing = !text.split_whitespace().any(|link| link == confirmed.url);
            Ok(confirmed)
        }
        Err(error) => lookup(target, repository, run)?.ok_or(error),
    }
}

#[cfg(test)]
mod tests;
