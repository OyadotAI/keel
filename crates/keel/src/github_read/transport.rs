//! Read-only GitHub CLI calls share one deadline and bounded output, with credentials only in env.
use super::constants::*;
use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;
use std::time::Instant;

/// A small read seam makes real GitHub pagination and error behavior hermetic in domain tests.
pub(crate) trait Read {
    fn rest(&mut self, path: &str) -> Result<Value, String>;
    fn graphql(&mut self, query: &str, variables: Value) -> Result<Value, String>;
}

/// One explicit action owns its deadline; pagination cannot renew it on each request.
pub(crate) struct Gh {
    root: Utf8PathBuf,
    deadline: Instant,
}
impl Gh {
    /// The repository only selects CLI context; API reads remain fixed to GitHub.com.
    pub(crate) fn new(root: &Utf8Path) -> Self {
        Self {
            root: root.to_owned(),
            deadline: Instant::now() + OPERATION_WAIT,
        }
    }

    fn run(&self, args: &[String]) -> Result<Value, String> {
        let mut command = std::process::Command::new(crate::permissions::program("gh"));
        command
            .current_dir(&self.root)
            .args(args)
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_PAGER", "cat");
        if std::env::var_os("GH_TOKEN").is_none()
            && let Some(token) =
                keel_providers::credentials::load(keel_providers::credentials::Kind::GitHub)
        {
            command.env("GH_TOKEN", token);
        }
        let output = crate::git::bounded_before(
            command,
            self.deadline.min(Instant::now() + REQUEST_WAIT),
            RESPONSE_BYTES,
        )
        .map_err(|e| {
            format!("Couldn’t read GitHub. Check GitHub CLI and sign-in, then retry. {e}")
        })?;
        if !output.status.success() {
            return Err(format!(
                "GitHub read failed: {}",
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(ERROR_CHARS)
                    .collect::<String>()
            ));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|_| "GitHub returned an incomplete or invalid response.".into())
    }

    /// Verify the actual source's fork relationship; a remote merely named upstream grants nothing.
    pub(crate) fn project(&mut self) -> Result<Project, String> {
        let (source, push_identity) = crate::pr::source_identity(&self.root)?;
        let context = self.run(&["repo", "view", "--json", "url"].map(str::to_string))?;
        if !context["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("https://github.com/"))
        {
            return Err("Issue tasks require a GitHub.com repository. Check this project's GitHub connection.".into());
        }
        let value = self.rest(&format!("repos/{source}"))?;
        let actual = text(&value, "full_name")?;
        if !actual.eq_ignore_ascii_case(&source) {
            return Err(
                "GitHub's repository identity changed. Update origin and preview again.".into(),
            );
        }
        let parent = value
            .get("parent")
            .and_then(|p| p["full_name"].as_str())
            .map(str::to_string);
        if parent
            .as_deref()
            .is_some_and(|name| !valid_repository(name))
        {
            return Err("GitHub returned an invalid parent repository.".into());
        }
        if parent.is_some() && value["fork"] != true {
            return Err("GitHub did not confirm this repository's fork relationship.".into());
        }
        Ok(Project {
            source: actual,
            parent,
            push_identity,
        })
    }
}

/// Hosting metadata keeps source and verified parent distinct without retaining remote credentials.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Project {
    pub source: String,
    pub parent: Option<String>,
    pub push_identity: String,
}

impl Read for Gh {
    fn rest(&mut self, path: &str) -> Result<Value, String> {
        if path.starts_with('/') || path.contains("://") || path.split('/').any(|part| part == "..")
        {
            return Err("Invalid GitHub read path.".into());
        }
        self.run(&[
            "api".into(),
            "--hostname".into(),
            "github.com".into(),
            "--method".into(),
            "GET".into(),
            "-H".into(),
            "Accept: application/vnd.github+json".into(),
            "-H".into(),
            format!("X-GitHub-Api-Version: {API_VERSION}"),
            path.into(),
        ])
    }
    fn graphql(&mut self, query: &str, variables: Value) -> Result<Value, String> {
        let mut args = vec![
            "api".into(),
            "--hostname".into(),
            "github.com".into(),
            "graphql".into(),
            "-f".into(),
            format!("query={query}"),
        ];
        for (key, value) in variables.as_object().ok_or("Invalid GraphQL variables.")? {
            if value.is_null() {
                continue;
            }
            args.push(
                if value.is_number() || value.is_boolean() {
                    "-F"
                } else {
                    "-f"
                }
                .into(),
            );
            args.push(format!(
                "{key}={}",
                value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string())
            ));
        }
        let response = self.run(&args)?;
        if response.get("errors").is_some() {
            return Err(
                "GitHub returned a partial GraphQL response. Previous feedback was retained."
                    .into(),
            );
        }
        Ok(response)
    }
}

pub(super) fn text(value: &Value, key: &str) -> Result<String, String> {
    value[key]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("GitHub omitted required field {key}."))
}
pub(super) fn valid_repository(value: &str) -> bool {
    let parts: Vec<_> = value.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && *p != "."
                && *p != ".."
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}
pub(super) fn clipped(value: &str) -> (String, bool) {
    let mut end = value.len().min(BODY_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_string(), end != value.len())
}
pub(super) fn link(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|url| {
            reqwest::Url::parse(url).is_ok_and(|u| {
                u.scheme() == "https" && u.username().is_empty() && u.password().is_none()
            })
        })
        .map(str::to_string)
}
