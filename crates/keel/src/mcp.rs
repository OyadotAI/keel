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
        // The other agents only after Claude's own add succeeded, and their result never changes
        // its code: a failed copy is said, the add itself still happened.
        if code == 0 && scope == "user" {
            let lines = crate::serve::blocking(move || mirror_add(&q), Vec::new()).await;
            send_lines(&tx, lines).await;
        }
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
        // Claude's entry before it goes: a copy elsewhere is removed only if it is this one.
        let before = if scope == "user" {
            let name = name.clone();
            crate::serve::blocking(
                move || {
                    keel_workspace::claude_home()
                        .and_then(|h| keel_workspace::user_mcp_entry(&h, &name))
                        .and_then(|v| Entry::from_json(&v))
                        // One with secrets was never copied, so nothing elsewhere is Keel's.
                        .filter(|e| *e != Entry::Carries)
                },
                None,
            )
            .await
        } else {
            None
        };
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
        if code == 0 && before.is_some() {
            let name = name.clone();
            let lines =
                crate::serve::blocking(move || mirror_remove(&name, before.as_ref()), Vec::new())
                    .await;
            send_lines(&tx, lines).await;
        }
        let _ = tx
            .send(Ok(Event::default().event("done").data(code.to_string())))
            .await;
    });
    Sse::new(ReceiverStream::new(rx))
}

// ── Mirroring to the other coding agents ─────────────────────────────────────────────────────
//
// A server added for every project (`--scope user`) is copied to Antigravity and Codex when they
// are installed, so all three agents work with the same tools. Only that scope: `agy mcp add` and
// `codex mcp add` write one global file each, so a project's server copied there would follow the
// person into every other project. And never a secret — a header or env value would land in a
// second global file, and neither CLI is known to expand `${NAME}`.

/// One of the other coding agents a user-scope server is copied to.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Antigravity,
    Codex,
}

impl Target {
    const ALL: [Target; 2] = [Target::Antigravity, Target::Codex];

    fn cli(self) -> &'static str {
        match self {
            Target::Antigravity => "agy",
            Target::Codex => "codex",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Target::Antigravity => "Antigravity",
            Target::Codex => "Codex",
        }
    }
}

/// Why a server is not copied to a target.
#[derive(Debug, PartialEq)]
enum Skip {
    Secret,
    NoSse,
}

/// A server as a target holds it: a command with its arguments, or a URL — or one carrying env
/// or header values, which Keel never copies, so it is never Keel's copy to remove.
#[derive(Debug, PartialEq, Clone)]
enum Entry {
    Stdio(String, Vec<String>),
    Url(String),
    Carries,
}

impl Entry {
    /// From a config entry: `{command, args}`, or a URL under any of the names the CLIs use.
    fn from_json(v: &serde_json::Value) -> Option<Entry> {
        let carries = |v: &serde_json::Value| {
            ["env", "headers", "http_headers", "env_http_headers"]
                .iter()
                .any(|k| v[*k].as_object().is_some_and(|o| !o.is_empty()))
        };
        if carries(v) || v.get("transport").is_some_and(carries) {
            return Some(Entry::Carries);
        }
        let v = v.get("transport").unwrap_or(v);
        if let Some(cmd) = v["command"].as_str() {
            let args = v["args"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            return Some(Entry::Stdio(cmd.to_string(), args));
        }
        ["url", "serverUrl", "httpUrl"]
            .iter()
            .find_map(|k| v[*k].as_str())
            .map(|u| Entry::Url(u.to_string()))
    }
}

/// The `<cli>` arguments that add `q` to `target`, or why it is not copied.
fn mirror_args(target: Target, q: &AddQuery) -> Result<Vec<String>, Skip> {
    let lines = |v: &Option<String>| -> Vec<String> {
        v.iter()
            .flat_map(|s| s.lines())
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    };
    let remote = matches!(q.transport.as_str(), "http" | "sse");
    if q.transport == "sse" {
        return Err(Skip::NoSse);
    }
    let headers = if remote {
        lines(&q.headers)
    } else {
        Vec::new()
    };
    let env = if remote { Vec::new() } else { lines(&q.env) };
    let target_text = q.target.trim();
    if !env.is_empty()
        || target_text.contains("${")
        || committed_secret(&q.transport, target_text).is_some()
    {
        return Err(Skip::Secret);
    }
    // Codex can read a bearer token from the environment itself; that one header is no secret.
    let bearer = match (target, headers.as_slice()) {
        (_, []) => None,
        (Target::Codex, [h]) => Some(
            h.strip_prefix("Authorization: Bearer ${")
                .and_then(|v| v.strip_suffix('}'))
                .filter(|v| {
                    !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                })
                .ok_or(Skip::Secret)?
                .to_string(),
        ),
        _ => return Err(Skip::Secret),
    };
    let mut args = vec!["mcp".to_string(), "add".to_string(), q.name.clone()];
    match (target, remote) {
        (Target::Codex, true) => {
            args.extend(["--url".to_string(), target_text.to_string()]);
            if let Some(var) = bearer {
                args.extend(["--bearer-token-env-var".to_string(), var]);
            }
        }
        (Target::Antigravity, true) => args.push(target_text.to_string()),
        (_, false) => {
            // `--` first, so a command's own flags are never read as the CLI's.
            args.push("--".into());
            args.extend(target_text.split_whitespace().map(str::to_string));
        }
    }
    Ok(args)
}

/// What `target` holds under `name`, if anything. Blocking.
fn existing(target: Target, name: &str) -> Option<Entry> {
    match target {
        // agy's own HOME, not KEEL_HOME: it does not read Keel's override, so asking the wrong
        // file would read "absent" and let `agy mcp add` overwrite the real entry.
        Target::Antigravity => {
            let file = std::env::home_dir()?.join(".gemini/config/mcp_config.json");
            let text = std::fs::read_to_string(file).ok()?;
            // agy reads comments and trailing commas this cannot. A file it cannot read may hold
            // the name, so it is treated as holding something that is not Keel's.
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                return text
                    .contains(&format!("\"{name}\""))
                    .then_some(Entry::Carries);
            };
            let entry = v["mcpServers"].get(name)?;
            Some(Entry::from_json(entry).unwrap_or(Entry::Carries))
        }
        Target::Codex => {
            let mut c = std::process::Command::new(crate::permissions::program("codex"));
            c.args(["mcp", "get", name, "--json"]);
            let out = crate::git::output_within(c, MIRROR_CEILING).ok()?;
            if !out.status.success() {
                return None;
            }
            Entry::from_json(&serde_json::from_slice(&out.stdout).ok()?)
        }
    }
}

/// How long one `agy`/`codex` config command may take. Each is a local file edit; past this it is
/// waiting on something (a login prompt) that Keel will not answer.
const MIRROR_CEILING: std::time::Duration = std::time::Duration::from_secs(30);

/// Run one CLI command, and the last line it printed to stderr when it failed.
fn run_cli(target: Target, args: &[String]) -> Result<(), String> {
    let mut c = std::process::Command::new(crate::permissions::program(target.cli()));
    c.args(args);
    match crate::git::output_within(c, MIRROR_CEILING) {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr)
            .lines()
            .rfind(|l| !l.trim().is_empty())
            .unwrap_or("it printed nothing")
            .to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// The lines for copying `q` to every installed target. Blocking.
fn mirror_add(q: &AddQuery) -> Vec<String> {
    let mut out = Vec::new();
    for t in Target::ALL {
        if !crate::permissions::usable_on_path(t.cli()) {
            continue;
        }
        let args = match mirror_args(t, q) {
            Ok(args) => args,
            Err(Skip::Secret) => {
                out.push(format!(
                    "Not added to {}: it carries env or header values, and Keel won't copy \
                     secrets into a second global file. Add it there yourself if you want it.",
                    t.name()
                ));
                continue;
            }
            Err(Skip::NoSse) => {
                out.push(format!(
                    "Not added to {0}: {0} has no SSE transport.",
                    t.name()
                ));
                continue;
            }
        };
        // ponytail: checked then added — a server of the same name created in between is
        // overwritten; both CLIs overwrite silently and neither offers an add-if-absent.
        if existing(t, &q.name).is_some() {
            out.push(format!(
                "Not added to {}: it already has a server named {}. Keel left that one alone.",
                t.name(),
                q.name
            ));
            continue;
        }
        out.push(match run_cli(t, &args) {
            Ok(()) => format!("Also added to {}.", t.name()),
            Err(why) => format!(
                "Not added to {}: `{} mcp add` failed: {why}",
                t.name(),
                t.cli()
            ),
        });
    }
    out
}

/// The lines for removing the copies of `claude` (Claude's entry, read before it was removed)
/// from every installed target. A same-named server that is not that entry is the person's own.
fn mirror_remove(name: &str, claude: Option<&Entry>) -> Vec<String> {
    let mut out = Vec::new();
    for t in Target::ALL {
        if !crate::permissions::usable_on_path(t.cli()) {
            continue;
        }
        let Some(theirs) = existing(t, name) else {
            continue;
        };
        if claude != Some(&theirs) || theirs == Entry::Carries {
            out.push(format!(
                "Left {}'s {name}: it isn't the one Keel added.",
                t.name()
            ));
            continue;
        }
        let args = ["mcp", "remove", name].map(String::from);
        out.push(match run_cli(t, &args) {
            Ok(()) => format!("Also removed from {}.", t.name()),
            Err(why) => format!(
                "Not removed from {}: `{} mcp remove` failed: {why}",
                t.name(),
                t.cli()
            ),
        });
    }
    out
}

async fn send_lines(tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>, lines: Vec<String>) {
    for line in lines {
        let _ = tx.send(Ok(Event::default().event("line").data(line))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(transport: &str, target: &str, headers: Option<&str>, env: Option<&str>) -> AddQuery {
        AddQuery {
            name: "qa".into(),
            transport: transport.into(),
            target: target.into(),
            scope: Some("user".into()),
            headers: headers.map(str::to_string),
            env: env.map(str::to_string),
            commit: false,
        }
    }

    /// A server is copied only when nothing secret would land in a second global file, and in the
    /// shape each CLI takes.
    #[test]
    fn a_user_server_is_copied_only_without_secrets() {
        let stdio = query("stdio", "npx -y @x/server --flag", None, None);
        assert_eq!(
            mirror_args(Target::Antigravity, &stdio).unwrap(),
            ["mcp", "add", "qa", "--", "npx", "-y", "@x/server", "--flag"]
        );
        assert_eq!(mirror_args(Target::Codex, &stdio).unwrap()[3], "--");
        for secret in [
            query("stdio", "npx s", None, Some("API_KEY=abc")),
            query("http", "https://h/mcp", Some("X-Key: abc"), None),
            query("http", "https://h/mcp?token=abc", None, None),
            query("stdio", "npx s --token ${T}", None, None),
        ] {
            assert_eq!(mirror_args(Target::Antigravity, &secret), Err(Skip::Secret));
            assert_eq!(mirror_args(Target::Codex, &secret), Err(Skip::Secret));
        }
        let sse = query("sse", "https://h/sse", None, None);
        assert_eq!(mirror_args(Target::Codex, &sse), Err(Skip::NoSse));
        let bearer = query(
            "http",
            "https://h/mcp",
            Some("Authorization: Bearer ${TOK}"),
            None,
        );
        assert_eq!(
            mirror_args(Target::Codex, &bearer).unwrap(),
            [
                "mcp",
                "add",
                "qa",
                "--url",
                "https://h/mcp",
                "--bearer-token-env-var",
                "TOK"
            ]
        );
        assert_eq!(mirror_args(Target::Antigravity, &bearer), Err(Skip::Secret));
    }

    /// Removal compares whole entries: two `npx` servers are not the same server.
    #[test]
    fn entries_compare_by_command_and_arguments() {
        let a = Entry::from_json(&serde_json::json!({"command": "npx", "args": ["-y", "a"]}));
        let b = Entry::from_json(&serde_json::json!({"command": "npx", "args": ["-y", "b"]}));
        assert_ne!(a, b);
        let codex = serde_json::json!({"transport": {"type": "stdio", "command": "npx", "args": ["-y", "a"]}});
        assert_eq!(Entry::from_json(&codex), a);
        // The person's own copy of a server Keel would not copy, with its token: never Keel's.
        let claude = serde_json::json!({"type": "http", "url": "https://h/mcp", "headers": {"Authorization": "Bearer x"}});
        let codex = serde_json::json!({"transport": {"type": "streamable_http", "url": "https://h/mcp", "http_headers": {"Authorization": "Bearer y"}}});
        assert_eq!(Entry::from_json(&claude), Some(Entry::Carries));
        assert_eq!(Entry::from_json(&codex), Some(Entry::Carries));
        let url = Entry::from_json(&serde_json::json!({"serverUrl": "https://h/mcp"}));
        assert_eq!(url, Some(Entry::Url("https://h/mcp".into())));
    }

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
