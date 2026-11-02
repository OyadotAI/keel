//! Hosting identity binds reconciliation to the intended base and the repository receiving pushes.
use serde::Deserialize;

/// Captured hosting names keep fork branches from impersonating the checkout's publication.
#[derive(Clone)]
pub(super) struct Repository {
    pub base: String,
    pub source: String,
    pub origin: String,
}

#[derive(Deserialize)]
struct Identity {
    #[serde(rename = "nameWithOwner")]
    name: String,
}

fn valid_name(name: &str) -> bool {
    let parts: Vec<_> = name.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}

fn source(origin: &str) -> Result<String, String> {
    let path = origin
        .strip_prefix("git@github.com:")
        .or_else(|| {
            let (scheme, rest) = origin.split_once("://")?;
            if !["https", "http", "ssh", "git"].contains(&scheme) {
                return None;
            }
            let (host, path) = rest.split_once('/')?;
            host.rsplit('@')
                .next()?
                .eq_ignore_ascii_case("github.com")
                .then_some(path)
        })
        .ok_or(
            "Could not identify origin's GitHub repository. Check its push URL before opening.",
        )?;
    let name = path
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_end_matches('/'));
    valid_name(name)
        .then(|| name.to_string())
        .ok_or("Origin's GitHub repository is invalid.".into())
}

/// Resolve once, without printing remote URLs that may contain authentication material.
pub(super) fn capture(
    run: &mut impl FnMut(&str, &[String]) -> Result<String, String>,
) -> Result<Repository, String> {
    let origin = run(
        "git",
        &["remote", "get-url", "--push", "--all", "origin"].map(str::to_string),
    )?;
    let source = source(&origin)?;
    let text = run(
        "gh",
        &["repo", "view", "--json", "nameWithOwner"].map(str::to_string),
    )?;
    let identity: Identity = serde_json::from_str(&text)
        .map_err(|_| "GitHub did not identify the pull request repository.")?;
    if !valid_name(&identity.name) {
        return Err("GitHub returned an invalid repository identity.".into());
    }
    Ok(Repository {
        base: identity.name,
        source,
        origin,
    })
}

impl Repository {
    /// Pin fork creation to its owner while ordinary same-repository branches retain their name.
    pub fn head(&self, branch: &str) -> String {
        if self.source.eq_ignore_ascii_case(&self.base) {
            branch.into()
        } else {
            format!(
                "{}:{branch}",
                self.source.split('/').next().unwrap_or_default()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_identity_accepts_github_transports_but_rejects_ambiguous_or_foreign_remotes() {
        for remote in [
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo.git",
            "ssh://git@github.com/owner/repo.git",
        ] {
            assert_eq!(source(remote).unwrap(), "owner/repo");
        }
        for remote in [
            "https://other.invalid/owner/repo",
            "https://github.com/owner/repo\nhttps://github.com/other/repo",
            "https://github.com/../repo",
            "file:///tmp/repo",
        ] {
            assert!(source(remote).is_err());
        }
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;
    #[test]
    fn fork_source_and_base_are_captured_independently_and_creation_qualifies_the_owner() {
        let repository = capture(&mut |program, args| {
            if program == "git" {
                return Ok("git@github.com:contributor/fork.git".into());
            }
            assert_eq!(args, ["repo", "view", "--json", "nameWithOwner"]);
            Ok(r#"{"nameWithOwner":"upstream/project"}"#.into())
        })
        .unwrap();
        assert_eq!(repository.base, "upstream/project");
        assert_eq!(repository.source, "contributor/fork");
        assert_eq!(repository.head("feature"), "contributor:feature");
    }
}
