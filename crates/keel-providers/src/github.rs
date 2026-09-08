//! GitHub: identity, repository listing, and cloning.

use crate::credentials;
use camino::Utf8Path;
use serde::{Deserialize, Serialize};

const API: &str = "https://api.github.com";
const UA: &str = "keel";

/// The authenticated account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

/// A repository the user can open.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub full_name: String,
    pub name: String,
    pub private: bool,
    pub description: Option<String>,
    pub clone_url: String,
    pub default_branch: Option<String>,
    pub updated_at: Option<String>,
    pub language: Option<String>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(UA)
        .build()
        .expect("building an HTTP client with no custom TLS config cannot fail")
}

/// Verify a token and return who it belongs to.
pub async fn verify(token: &str) -> Result<Account, String> {
    let response = client()
        .get(format!("{API}/user"))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("could not reach GitHub: {e}"))?;

    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("that token was rejected — check it has not expired".into());
    }
    if !response.status().is_success() {
        return Err(format!("GitHub returned {}", response.status()));
    }

    response
        .json::<Account>()
        .await
        .map_err(|e| format!("unexpected response from GitHub: {e}"))
}

/// Repositories the user can push to, most recently updated first.
///
/// Sorted by update time rather than name because the repository someone wants to open is
/// overwhelmingly one they touched recently.
pub async fn repos(token: &str) -> Result<Vec<Repo>, String> {
    // `organization_member` was missing, so an account whose work is in organisations — which is
    // every work account — searched a list its own repositories were not in. It is part of the
    // endpoint's default and was lost by naming the other two.
    //
    // Three pages, not one: 100 was a single page of a list sorted by update time, so the older
    // half of a person's organisations simply did not exist here. Three is a ceiling rather than a
    // guess at a maximum — the list is a picker, and 300 entries sorted by recency is already more
    // than anyone scrolls; searching is what finds the rest.
    let mut all: Vec<Repo> = Vec::new();
    for page in 1..=3 {
        let response = client()
            .get(format!("{API}/user/repos"))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .query(&[
                ("sort", "updated"),
                ("per_page", "100"),
                ("page", &page.to_string()),
                ("affiliation", "owner,collaborator,organization_member"),
            ])
            .send()
            .await
            .map_err(|e| format!("could not reach GitHub: {e}"))?;

        if !response.status().is_success() {
            // A page that fails after one that worked is a partial answer, not a failure: the
            // picker is more useful with the first hundred in it than with an error over it.
            if all.is_empty() {
                return Err(format!("GitHub returned {}", response.status()));
            }
            break;
        }

        let batch = response
            .json::<Vec<Repo>>()
            .await
            .map_err(|e| format!("unexpected response from GitHub: {e}"))?;
        let short = batch.len() < 100;
        all.extend(batch);
        if short {
            break;
        }
    }
    Ok(all)
}

/// A credential helper that answers with the token Keel already holds, for this one `git` and
/// nothing else.
///
/// The token travels in the environment, never on the command line — an argument list is world
/// readable through `ps` — and never into `.git/config`, which an embedded `https://token@host`
/// URL would leave on disk in the clone. The empty helper first resets the ones git would
/// otherwise inherit, so ours is the one that answers.
///
/// Installed under `credential.https://github.com.helper`, which is git's own per-URL form: a
/// helper installed as plain `credential.helper` answers for *every* host, and the URL reaching
/// this function arrives over loopback, where any page in a browser can post one. A redirect or a
/// crafted `clone_url` would then have git hand the person's GitHub token to whoever asked.
/// `is_github_https` refuses the URL as well; this is the half that holds if a redirect gets past
/// it.
const CREDENTIAL_HELPER: &str = "!f() { test \"$1\" = get && printf 'username=x-access-token\\npassword=%s\\n' \"$KEEL_GITHUB_TOKEN\"; }; f";

/// Whether this is a URL Keel will hand a GitHub token to.
///
/// `/api/github/clone` is reachable from any page in the browser — loopback is not a boundary —
/// so the URL is somebody's input rather than the API's answer. A leading `-` is refused as well:
/// `git clone` would read it as a flag rather than a URL.
fn is_github_https(url: &str) -> bool {
    !url.starts_with('-')
        && (url.starts_with("https://github.com/") || url.starts_with("https://www.github.com/"))
}

/// Clone `url` into `parent/name`, returning the new path.
///
/// Shells out to `git` rather than linking a git library: the user's own git already has their
/// SSH agent, proxy settings and hooks configured, and reimplementing that is how a tool ends up
/// unable to clone from a private host.
///
/// What it does *not* inherit is the credentials. A private repository over HTTPS with no helper
/// configured makes git ask for a username, and a `git` with no terminal under it fails with
/// `could not read Username for 'https://github.com': Device not configured` — which is what a
/// tester saw under a panel promising the clone used their `gh` credentials. It does now: the
/// token Keel is already signed in with is handed to git for this one command.
pub fn clone(url: &str, parent: &Utf8Path, name: &str) -> Result<camino::Utf8PathBuf, String> {
    if !is_github_https(url) {
        return Err(format!("{url} is not a GitHub HTTPS URL"));
    }
    let target = parent.join(name);
    if target.exists() {
        return Err(format!("{target} already exists"));
    }

    let mut command = std::process::Command::new("git");
    command
        .current_dir(parent)
        // Never interactive. Without this a missing credential is a prompt, and a prompt in a
        // process nobody can see is a request that waits for ever rather than an error anyone can
        // read. `GIT_ASKPASS` covers the graphical prompt the same way.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("GCM_INTERACTIVE", "never");

    if let Some(token) = credentials::github_token() {
        command
            .env("KEEL_GITHUB_TOKEN", token)
            .args(["-c", "credential.helper="])
            .args([
                "-c",
                &format!("credential.https://github.com.helper={CREDENTIAL_HELPER}"),
            ]);
    }

    let out = command
        .args(["clone", "--depth", "1", url, name])
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // Name the cause rather than passing git's word for it along. "could not read Username"
        // means Keel had no credential to give, and the fix for that is one screen away.
        if credentials::github_token().is_none() && stderr.contains("could not read Username") {
            return Err(format!(
                "{name} is private, and Keel is not signed in to GitHub. \
                 Connect it under Settings › Tools, then clone again."
            ));
        }
        return Err(stderr);
    }
    Ok(target)
}

/// The currently connected account, if any.
pub async fn current() -> Option<Account> {
    let token = credentials::github_token()?;
    verify(&token).await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The token is only ever offered to GitHub. The URL arrives over loopback, which any page in
    /// a browser can post to, so "it came from our own API" is not something this can assume.
    #[test]
    fn a_clone_url_that_is_not_github_over_https_is_refused() {
        assert!(is_github_https("https://github.com/OyadotAI/keel.git"));
        assert!(is_github_https("https://www.github.com/OyadotAI/keel"));

        assert!(!is_github_https("https://github.com.evil.example/x/y.git"));
        assert!(!is_github_https("https://evil.example/x/y.git"));
        assert!(!is_github_https("http://github.com/x/y.git"));
        assert!(!is_github_https("git@github.com:x/y.git"));
        assert!(!is_github_https("--upload-pack=touch /tmp/pwn"));
        assert!(!is_github_https("file:///etc/passwd"));
    }
}
