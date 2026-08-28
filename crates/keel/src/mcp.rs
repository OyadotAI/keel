//! Adding and removing MCP servers.
//!
//! Driven through `claude mcp` rather than by writing config files. Claude Code owns three scopes
//! with three different files and its own precedence between them; a second writer would be a
//! second opinion about where a server lives, and the two would disagree the first time someone
//! ran the CLI directly.
//!
//! Note what this does *not* touch: a server that arrived with the repository. Those are quarantined
//! and shown as such, and the way to accept one is to review it, not to press a button next to it.

use axum::extract::Query;
use axum::response::sse::{Event, Sse};
use serde::Deserialize;
use std::convert::Infallible;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
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
}

#[derive(Deserialize)]
pub struct RemoveQuery {
    pub name: String,
    pub scope: Option<String>,
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

fn scope_of(scope: &Option<String>) -> &'static str {
    match scope.as_deref() {
        Some("user") => "user",
        // Writing to `project` scope puts the server in `.mcp.json`, which is the file Keel
        // quarantines on trust. Offering it would mean handing someone a way to add a server that
        // Keel then treats as untrusted repository content.
        _ => "local",
    }
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

/// Stream a command's output, both streams interleaved in the order they arrive.
async fn pipe(
    command: &mut Command,
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) -> i32 {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let Ok(mut child) = command.spawn() else {
        let _ = tx
            .send(Ok(Event::default()
                .event("line")
                .data("could not run `claude` — is the CLI on your PATH?")))
            .await;
        return 1;
    };

    let mut out = BufReader::new(child.stdout.take().expect("piped")).lines();
    let mut err = BufReader::new(child.stderr.take().expect("piped")).lines();
    loop {
        tokio::select! {
            Ok(Some(line)) = out.next_line() => {
                let _ = tx.send(Ok(Event::default().event("line").data(line))).await;
            }
            Ok(Some(line)) = err.next_line() => {
                let _ = tx.send(Ok(Event::default().event("line").data(line))).await;
            }
            else => break,
        }
    }
    child.wait().await.ok().and_then(|s| s.code()).unwrap_or(1)
}

pub async fn add(Query(q): Query<AddQuery>) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    if !valid_name(&q.name) {
        return refuse("A name can hold letters, digits, hyphens and underscores.");
    }
    if q.target.trim().is_empty() {
        return refuse("A URL or command is required.");
    }

    let scope = scope_of(&q.scope);
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

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(async move {
        let _ = tx
            .send(Ok(Event::default()
                .event("line")
                .data(format!("$ claude {}", args.join(" ")))))
            .await;
        let code = pipe(Command::new("claude").args(&args), &tx).await;
        let _ = tx
            .send(Ok(Event::default().event("done").data(code.to_string())))
            .await;
    });
    Sse::new(ReceiverStream::new(rx))
}

pub async fn remove(
    Query(q): Query<RemoveQuery>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    if !valid_name(&q.name) {
        return refuse("invalid server name");
    }
    let scope = scope_of(&q.scope).to_string();
    let name = q.name.clone();

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(async move {
        let _ = tx
            .send(Ok(Event::default()
                .event("line")
                .data(format!("$ claude mcp remove --scope {scope} {name}"))))
            .await;
        let code = pipe(
            Command::new("claude").args(["mcp", "remove", "--scope", &scope, &name]),
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

    /// `project` scope writes `.mcp.json`, which is the file `keel-harness::trust` quarantines as
    /// untrusted repository content. Offering it would hand someone a button that adds a server
    /// Keel then refuses to load.
    #[test]
    fn project_scope_is_never_written() {
        assert_eq!(scope_of(&Some("user".into())), "user");
        assert_eq!(scope_of(&Some("local".into())), "local");
        assert_eq!(scope_of(&Some("project".into())), "local");
        assert_eq!(scope_of(&None), "local");
    }
}
