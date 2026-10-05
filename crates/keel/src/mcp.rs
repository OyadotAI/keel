//! Adding and removing MCP servers.
//!
//! Driven through `claude mcp` rather than by writing config files. Claude Code owns three scopes
//! with three different files and its own precedence between them; a second writer would be a
//! second opinion about where a server lives, and the two would disagree the first time someone
//! ran the CLI directly.
//!
//! Note what this does *not* touch: a server that arrived with the repository. Those are quarantined
//! and shown as such, and the way to accept one is to review it, not to press a button next to it.
//!
//! A server added here goes into the project by default — `.mcp.json`, committed — so it belongs
//! to the repository rather than to this machine. Keel records what it wrote (`vouched`), which is
//! what keeps its own `.mcp.json` out of quarantine here while a teammate's clone still asks. And
//! because that file is committed, a header or env value must be a `${VAR}` reference: a token
//! typed into the form would otherwise be pushed with it.

use axum::extract::{Query, State};
use axum::response::sse::{Event, Sse};
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

#[derive(Deserialize)]
pub struct AddQuery {
    pub name: String,
    /// `http`, `sse`, or `stdio`.
    pub transport: String,
    /// A URL for http/sse, or the command line for stdio.
    pub target: String,
    /// `local`, `user` or `project`.
    pub scope: Option<String>,
    /// `Name: value`, one per line. Sent for http and sse only.
    pub headers: Option<String>,
    /// `KEY=value`, one per line. Sent for stdio only.
    pub env: Option<String>,
    /// Commit `.mcp.json` after, for a project-scope server.
    #[serde(default)]
    pub commit: bool,
}

#[derive(Deserialize)]
pub struct RemoveQuery {
    pub name: String,
    pub scope: Option<String>,
    #[serde(default)]
    pub commit: bool,
}

/// A server name that is safe to pass as an argument and to write into a config key.
///
/// The leading-hyphen rule is the one that matters: hyphens are legal inside a name, so without it
/// `--transport` is a valid name, and `claude mcp add … --transport …` would read it as a flag
/// rather than as the thing it was typed into. Caught by the test below before it shipped.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Project unless asked otherwise: what Keel installs belongs to the project. `.mcp.json` is a file
/// Keel quarantines, and `vouched` is what lets the one Keel wrote stay.
fn scope_of(scope: &Option<String>) -> &'static str {
    match scope.as_deref() {
        Some("user") => "user",
        Some("local") => "local",
        _ => "project",
    }
}

/// A whole `${NAME}` — nothing else in the value, so nothing typed rides along beside it.
fn reference(token: &str) -> bool {
    token
        .strip_prefix("${")
        .and_then(|t| t.strip_suffix('}'))
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// A header or env value that would be committed as typed. Only a `${NAME}` reference may go into
/// `.mcp.json` — after an auth scheme word, for a header — because Claude Code expands it from the
/// environment of whoever runs the server, and anything literal is pushed with the repository.
fn literal_secret(lines: &Option<String>, separator: char) -> Option<String> {
    const SCHEMES: &[&str] = &["Bearer", "Basic", "Token", "Bot"];
    lines.iter().flat_map(|l| l.lines()).find_map(|line| {
        let (key, value) = line.split_once(separator)?;
        let mut words = value.split_whitespace().peekable();
        if separator == ':' && words.peek().is_some_and(|w| SCHEMES.contains(w)) {
            words.next();
        }
        let words: Vec<&str> = words.collect();
        let fine = words.is_empty() || (words.len() == 1 && reference(words[0]));
        (!fine).then(|| key.trim().to_string())
    })
}

/// Anything else in a project-scope server that would commit a credential: a URL carrying a
/// query or a user, or a command-line argument that is a secret's value.
fn committed_secret(transport: &str, target: &str) -> Option<String> {
    const SECRET: &[&str] = &["key", "token", "secret", "password", "passwd", "auth"];
    let secretish = |flag: &str| {
        let f = flag.to_ascii_lowercase();
        f.starts_with('-') && SECRET.iter().any(|s| f.contains(s))
    };
    if matches!(transport, "http" | "sse") {
        let rest = target.split_once("://").map_or(target, |(_, r)| r);
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.contains('@') {
            return Some("the URL's user and password".into());
        }
        if target.contains('?') {
            return Some("the URL's query string".into());
        }
        return None;
    }
    let args: Vec<&str> = target.split_whitespace().collect();
    for (i, arg) in args.iter().enumerate() {
        if let Some((flag, value)) = arg.split_once('=')
            && secretish(flag)
            && !reference(value)
        {
            return Some(format!("the value of {flag}"));
        }
        // `env GITHUB_TOKEN=ghp_… npx server`, or the assignment alone before the command: a
        // variable set inline is a literal in the committed file just as a flag's value is.
        if let Some((name, value)) = arg.split_once('=')
            && !name.starts_with('-')
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && SECRET.iter().any(|s| name.to_ascii_lowercase().contains(s))
            && !value.is_empty()
            && !reference(value)
        {
            return Some(format!("the value of {name}"));
        }
        if secretish(arg)
            && !arg.contains('=')
            && args
                .get(i + 1)
                .is_some_and(|v| !v.starts_with('-') && !reference(v))
        {
            return Some(format!("the value after {arg}"));
        }
    }
    None
}

fn refuse(message: &str) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let message = message.to_string();
    tokio::spawn(async move {
        let _ = tx
            .send(Ok(Event::default().event("line").data(message)))
            .await;
        let _ = tx.send(Ok(Event::default().event("done").data("1"))).await;
    });
    Sse::new(ReceiverStream::new(rx))
}

pub async fn add(
    State(state): State<Arc<crate::serve::AppState>>,
    Query(q): Query<AddQuery>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    // no-blocking: input validation in memory; the `claude mcp` process runs in a spawned task.
    if !valid_name(&q.name) {
        return refuse("A name can hold letters, digits, hyphens and underscores.");
    }
    if q.target.trim().is_empty() {
        return refuse("A URL or command is required.");
    }

    let scope = scope_of(&q.scope);
    if scope == "project" {
        let literal = literal_secret(&q.headers, ':')
            .or_else(|| literal_secret(&q.env, '='))
            .or_else(|| committed_secret(&q.transport, q.target.trim()));
        if let Some(key) = literal {
            return refuse(&format!(
                "{key} would be committed in .mcp.json as typed. Put the value in your \
                 environment and write it as ${{NAME}} (e.g. `Bearer ${{LINEAR_TOKEN}}`), or add \
                 the server to this machine only."
            ));
        }
    }
    let mut args: Vec<String> = vec!["mcp".into(), "add".into(), "--scope".into(), scope.into()];

    let remote = matches!(q.transport.as_str(), "http" | "sse");
    if remote {
        args.push("--transport".into());
        args.push(q.transport.clone());
        for header in q.headers.iter().flat_map(|h| h.lines()) {
            let header = header.trim();
            if !header.is_empty() {
                args.push("--header".into());
                args.push(header.to_string());
            }
        }
    } else {
        for pair in q.env.iter().flat_map(|e| e.lines()) {
            let pair = pair.trim();
            if !pair.is_empty() {
                args.push("--env".into());
                args.push(pair.to_string());
            }
        }
    }

    args.push(q.name.clone());

    if remote {
        args.push(q.target.trim().to_string());
    } else {
        // `--` first, so a command's own flags are never read as `claude mcp add`'s. The split is
        // whitespace only: anything needing quoting belongs in a wrapper script, and guessing at
        // shell quoting here would be a way to run something nobody typed.
        args.push("--".into());
        args.extend(q.target.split_whitespace().map(str::to_string));
    }

    // `--scope local` means "this project", and Claude Code decides which project that is from the
    // working directory — not from any argument. Keel launched from the Dock inherits launchd's
    // cwd, which is `/`, so every server added through the UI was filed under a project called `/`
    // and then never appeared: the panel reads the servers for the repository, and `/` is not it.
    // Running the CLI in the repository is the whole fix.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    let message = format!("Keel: add MCP server {}", q.name);
    tokio::spawn(async move {
        let code = crate::vouched::run(
            &state,
            ".mcp.json",
            scope == "project",
            &args,
            q.commit,
            &message,
            &tx,
        )
        .await;
        let _ = tx
            .send(Ok(Event::default().event("done").data(code.to_string())))
            .await;
    });
    Sse::new(ReceiverStream::new(rx))
}

pub async fn remove(
    State(state): State<Arc<crate::serve::AppState>>,
    Query(q): Query<RemoveQuery>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    // no-blocking: input validation in memory; the `claude mcp` process runs in a spawned task.
    if !valid_name(&q.name) {
        return refuse("invalid server name");
    }
    let scope = scope_of(&q.scope);
    let name = q.name.clone();

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(async move {
        let args: Vec<String> = ["mcp", "remove", "--scope", scope, &name]
            .map(String::from)
            .to_vec();
        let message = format!("Keel: remove MCP server {name}");
        let code = crate::vouched::run(
            &state,
            ".mcp.json",
            scope == "project",
            &args,
            q.commit,
            &message,
            &tx,
        )
        .await;
        let _ = tx
            .send(Ok(Event::default().event("done").data(code.to_string())))
            .await;
    });
    Sse::new(ReceiverStream::new(rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `.mcp.json` is committed, so only references to the environment may go into it.
    #[test]
    fn a_typed_token_is_refused_at_project_scope() {
        let one = |h: &str| literal_secret(&Some(h.to_string()), ':');
        assert_eq!(
            one("Authorization: Bearer sk-live-123").as_deref(),
            Some("Authorization")
        );
        assert_eq!(one("Authorization: Bearer ${LINEAR_TOKEN}"), None);
        // A reference beside a literal still commits the literal.
        assert!(one("Authorization: Bearer sk-live-123 ${X}").is_some());
        assert!(one("X-Key: pre${X}").is_some());
        let env = Some("API_KEY=${API_KEY}\nREGION=eu".to_string());
        assert_eq!(literal_secret(&env, '=').as_deref(), Some("REGION"));

        assert!(committed_secret("http", "https://h/mcp?token=abc").is_some());
        assert!(committed_secret("http", "https://u:p@h/mcp").is_some());
        assert!(committed_secret("http", "https://mcp.linear.app/mcp").is_none());
        assert!(committed_secret("stdio", "npx srv --api-key sk-123").is_some());
        assert!(committed_secret("stdio", "npx srv --token=sk-123").is_some());
        assert!(committed_secret("stdio", "npx srv --api-key ${KEY}").is_none());
        assert!(committed_secret("stdio", "env GITHUB_TOKEN=ghp_abc123 npx server").is_some());
        assert!(committed_secret("stdio", "env GITHUB_TOKEN=${GITHUB_TOKEN} npx server").is_none());
        assert!(committed_secret("stdio", "env NODE_ENV=production npx server").is_none());
        assert!(committed_secret("stdio", "npx -y @modelcontextprotocol/server-github").is_none());
    }

    #[test]
    fn names_are_bounded_to_what_is_safe_as_an_argument() {
        assert!(valid_name("sentry"));
        assert!(valid_name("my_server-2"));
        // A hyphen is legal inside a name but never at the front, or the name is a flag.
        assert!(!valid_name("--transport"));
        assert!(!valid_name("-s"));
        assert!(!valid_name("a b"));
        assert!(!valid_name("rm -rf /"));
        assert!(!valid_name(""));
    }

    /// Project is the default and is honoured: `.mcp.json` is quarantined as repository content,
    /// and `vouched` is what keeps the one Keel wrote loading here. This test used to pin the
    /// opposite, from before there was a way to tell Keel's copy from a repository's.
    #[test]
    fn project_scope_is_the_default_and_is_honoured() {
        assert_eq!(scope_of(&Some("user".into())), "user");
        assert_eq!(scope_of(&Some("local".into())), "local");
        assert_eq!(scope_of(&Some("project".into())), "project");
        assert_eq!(scope_of(&None), "project");
    }
}
