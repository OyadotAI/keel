use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

/// Which deployment environment the agent is acting against.
///
/// This is the autonomy control, not a label. In `Dev` the agent acts freely; in `Prod` every
/// mutating tool call requires human approval. Carrying it in the invocation means the distinction
/// is enforced where the process is built, not remembered later by a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Dev,
    Prod,
}

impl Environment {
    pub fn as_str(self) -> &'static str {
        match self {
            Environment::Dev => "dev",
            Environment::Prod => "prod",
        }
    }

    /// Whether a mutating tool call needs a human before it runs.
    pub fn requires_approval(self) -> bool {
        matches!(self, Environment::Prod)
    }
}

/// A single `claude -p` run, assembled rather than string-formatted so the argument list is
/// inspectable and testable.
#[derive(Debug, Clone)]
pub struct Invocation {
    prompt: String,
    cwd: Utf8PathBuf,
    mcp_config: Utf8PathBuf,
    system_prompt_file: Option<Utf8PathBuf>,
    resume: Option<String>,
    environment: Environment,
}

impl Invocation {
    pub fn new(
        prompt: impl Into<String>,
        cwd: impl AsRef<Utf8Path>,
        mcp_config: impl AsRef<Utf8Path>,
        environment: Environment,
    ) -> Self {
        Self {
            prompt: prompt.into(),
            cwd: cwd.as_ref().to_owned(),
            mcp_config: mcp_config.as_ref().to_owned(),
            system_prompt_file: None,
            resume: None,
            environment,
        }
    }

    pub fn with_system_prompt_file(mut self, path: impl AsRef<Utf8Path>) -> Self {
        self.system_prompt_file = Some(path.as_ref().to_owned());
        self
    }

    /// Continue an existing session by id, so a conversation survives across turns.
    pub fn resuming(mut self, session_id: impl Into<String>) -> Self {
        self.resume = Some(session_id.into());
        self
    }

    pub fn cwd(&self) -> &Utf8Path {
        &self.cwd
    }

    pub fn environment(&self) -> Environment {
        self.environment
    }

    /// The full argument list, in order.
    ///
    /// Two flags carry the guardrails and must never be dropped:
    ///
    /// - `--permission-mode dontAsk` makes Claude Code deny anything not explicitly allowed. Since
    ///   Keel allows nothing built-in, the agent can only act through Keel's MCP tools. An agent
    ///   that cannot call `bash` cannot delete a database.
    /// - `--strict-mcp-config` keeps the session to the MCP servers Keel passes, ignoring any the
    ///   repository or the user's global config would otherwise contribute.
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "-p".into(),
            self.prompt.clone(),
            // stream-json is what the bridge parses into UI frames; --verbose and
            // --include-partial-messages are required to get incremental text rather than one
            // final block.
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--include-partial-messages".into(),
            "--mcp-config".into(),
            self.mcp_config.to_string(),
            "--strict-mcp-config".into(),
            "--permission-mode".into(),
            "dontAsk".into(),
            // Subagent attribution: the bridge uses parent_tool_use_id to render the agent tree.
            "--forward-subagent-text".into(),
        ];

        if let Some(path) = &self.system_prompt_file {
            args.push("--append-system-prompt-file".into());
            args.push(path.to_string());
        }

        if let Some(session) = &self.resume {
            args.push("--resume".into());
            args.push(session.clone());
        }

        args
    }
}

/// The MCP server block Keel writes for the CLI to load.
///
/// Keel's tools are served over loopback by the `keel-mcp` crate in this same process.
#[derive(Debug, Serialize)]
pub struct McpConfig {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: serde_json::Map<String, serde_json::Value>,
}

impl McpConfig {
    /// Point the CLI at Keel's loopback MCP endpoint, authenticated with a per-session token.
    pub fn loopback(port: u16, session_token: &str) -> Self {
        let mut servers = serde_json::Map::new();
        servers.insert(
            "keel".to_string(),
            serde_json::json!({
                "type": "http",
                "url": format!("http://127.0.0.1:{port}/mcp"),
                "headers": { "Authorization": format!("Bearer {session_token}") }
            }),
        );
        Self {
            mcp_servers: servers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation() -> Invocation {
        Invocation::new("do the thing", "/repo", "/tmp/mcp.json", Environment::Dev)
    }

    #[test]
    fn never_passes_bare_which_would_break_subscription_auth() {
        assert!(!invocation().args().iter().any(|a| a == "--bare"));
    }

    #[test]
    fn locks_the_tool_surface() {
        let args = invocation().args();
        let mode = args
            .iter()
            .position(|a| a == "--permission-mode")
            .expect("mode flag");
        assert_eq!(args[mode + 1], "dontAsk");
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
    }

    #[test]
    fn requests_a_parseable_event_stream() {
        let args = invocation().args();
        let fmt = args
            .iter()
            .position(|a| a == "--output-format")
            .expect("format flag");
        assert_eq!(args[fmt + 1], "stream-json");
        assert!(args.iter().any(|a| a == "--verbose"));
        assert!(args.iter().any(|a| a == "--include-partial-messages"));
    }

    #[test]
    fn resume_is_absent_unless_requested() {
        assert!(!invocation().args().iter().any(|a| a == "--resume"));
        let resumed = invocation().resuming("abc-123");
        let args = resumed.args();
        let idx = args
            .iter()
            .position(|a| a == "--resume")
            .expect("resume flag");
        assert_eq!(args[idx + 1], "abc-123");
    }

    #[test]
    fn prod_requires_approval_and_dev_does_not() {
        assert!(Environment::Prod.requires_approval());
        assert!(!Environment::Dev.requires_approval());
    }

    #[test]
    fn mcp_config_targets_loopback_only() {
        let config = McpConfig::loopback(7777, "tok");
        let json = serde_json::to_value(&config).expect("serialises");
        let url = json["mcpServers"]["keel"]["url"].as_str().expect("url");
        assert!(
            url.starts_with("http://127.0.0.1:"),
            "must not be reachable off-host: {url}"
        );
    }
}
