//! Only authenticated loopback MCP exports whose listener is the selected Oya installation are used.
use super::detection::{self, Installation};
use camino::Utf8Path;
use serde_json::Value;
use std::time::Duration;
const TOKEN_BYTES: usize = 4096;
const CONFIGURED_LIMIT: usize = 64;
const RESPONSE_BYTES: usize = 128 * 1024;
const CONNECT_WAIT: Duration = Duration::from_secs(5);
const CALL_WAIT: Duration = Duration::from_secs(20);

pub(super) struct Connection {
    pub url: reqwest::Url,
    authorization: String,
    pub process: u32,
    pub installation: Installation,
}
fn candidate(value: &Value, installation: &Installation) -> Result<Connection, String> {
    candidate_with(value, installation, detection::listener)
}
fn candidate_with(
    value: &Value,
    installation: &Installation,
    listener: impl FnOnce(u16, &Installation) -> Result<u32, String>,
) -> Result<Connection, String> {
    let url = reqwest::Url::parse(value["url"].as_str().ok_or("Missing local Oya URL")?)
        .map_err(|_| "Invalid local Oya connection URL")?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.path() != "/mcp"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Oya connection must use its verified loopback MCP endpoint.".into());
    }
    let authorization = value
        .pointer("/headers/Authorization")
        .or_else(|| value.pointer("/headers/authorization"))
        .or_else(|| value.pointer("/http_headers/Authorization"))
        .or_else(|| value.pointer("/http_headers/authorization"))
        .and_then(Value::as_str)
        .ok_or("Oya connection needs an exported bearer credential")?;
    let token = authorization
        .strip_prefix("Bearer ")
        .ok_or("Oya connection needs bearer authentication")?;
    if token.is_empty() || token.len() > TOKEN_BYTES || token.chars().any(char::is_control) {
        return Err("Oya bearer configuration is malformed.".into());
    }
    let process = listener(url.port().ok_or("Missing local Oya port")?, installation)?;
    Ok(Connection {
        url,
        authorization: authorization.into(),
        process,
        installation: installation.clone(),
    })
}
/// Server aliases are discovery hints; URL, authentication and process ownership establish readiness.
pub(super) fn discover(
    project: &Utf8Path,
    installation: &Installation,
) -> Result<Connection, String> {
    let home = keel_workspace::claude_home().ok_or("No provider configuration directory")?;
    let mut candidates = keel_workspace::discover_mcp_servers(project, &home)
        .into_iter()
        .filter(|s| s.scope == keel_workspace::Scope::User)
        .take(CONFIGURED_LIMIT)
        .filter_map(|s| keel_workspace::user_mcp_entry(&home, &s.name))
        .collect::<Vec<_>>();
    if let Some(connection) = candidates
        .iter()
        .find_map(|v| candidate(v, installation).ok())
    {
        return Ok(connection);
    }
    if crate::permissions::find_on_path("codex").is_some()
        && let Ok(config) = crate::provider_config::codex(project)
        && let Some(servers) = config["mcp_servers"].as_object()
    {
        candidates.extend(servers.values().take(CONFIGURED_LIMIT).cloned());
    }
    candidates.iter().find_map(|v|candidate(v,installation).ok()).ok_or("Oya Browser is installed; its agent connection needs setup. In Oya’s footer, use Connect Claude Code or Connect Codex, then Retry connection. Keel does not copy or decrypt login credentials.".into())
}
/// What a call says when Oya restarted under it. `local::Page` matches it to drop the stale
/// connection, so the next browser step reconnects on its own.
pub(super) const RESTARTED: &str = "Oya Browser restarted, so this lane's tab is gone. Keel reconnects on the next browser step; open the page again with browser_open.";
/// A distinct MCP session owns each lane's tabs, and redirects/proxies cannot forward its bearer.
pub(super) struct Client {
    connection: Connection,
    http: reqwest::Client,
    session: Option<String>,
    next: u64,
}
impl Client {
    pub fn new(connection: Connection) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_WAIT)
            .timeout(CALL_WAIT)
            .build()
            .map_err(|_| "Couldn’t create the Oya connection client.")?;
        Ok(Self {
            connection,
            http,
            session: None,
            next: 0,
        })
    }
    async fn ownership(&self) -> Result<(), String> {
        let port = self.connection.url.port().unwrap();
        let installation = self.connection.installation.clone();
        let expected = self.connection.process;
        let actual =
            crate::serve::in_blocking(move || detection::listener(port, &installation)).await?;
        if actual != expected {
            return Err(RESTARTED.into());
        }
        Ok(())
    }
    /// End this client's Oya session (MCP's DELETE with its id), so a status check or a reset page
    /// does not leave one open in Oya for each time it ran. Best effort: Oya may already be gone.
    pub async fn close(self) {
        if let Some(session) = &self.session {
            let _ = self
                .http
                .delete(self.connection.url.clone())
                .header("Authorization", &self.connection.authorization)
                .header("Mcp-Session-Id", session)
                .send()
                .await;
        }
    }
    pub async fn initialize(&mut self) -> Result<(), String> {
        self.rpc("initialize",serde_json::json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"Keel","version":env!("CARGO_PKG_VERSION")}})).await?;
        if self.session.is_none() {
            return Err("Oya did not provide a session identity; no tab was opened.".into());
        }
        self.rpc("ping", serde_json::json!({})).await?;
        let tools = self.rpc("tools/list", serde_json::json!({})).await?;
        for required in [
            "open_tab",
            "navigate",
            "switch_tab",
            "analyze_page",
            "click",
            "type",
            "press_key",
            "scroll",
            "go_back",
        ] {
            if !tools["tools"]
                .as_array()
                .is_some_and(|list| list.iter().any(|t| t["name"] == required))
            {
                return Err(format!(
                    "This Oya version does not provide {required}. Update Oya or choose Inside Keel."
                ));
            }
        }
        Ok(())
    }
    pub async fn tool(&mut self, name: &str, args: Value) -> Result<String, String> {
        let response = self
            .rpc(
                "tools/call",
                serde_json::json!({"name":name,"arguments":args}),
            )
            .await?;
        let text = response["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if response["isError"] == true {
            return Err(text);
        }
        Ok(text)
    }
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.ownership().await?;
        self.next = self.next.saturating_add(1);
        let id = self.next;
        let mut request = self
            .http
            .post(self.connection.url.clone())
            .header("Authorization", &self.connection.authorization)
            .header("Accept", "application/json, text/event-stream")
            .json(&serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        if let Some(session) = &self.session {
            request = request.header("Mcp-Session-Id", session);
        }
        let mut response=request.send().await.map_err(|_|"Local Oya did not confirm the operation. Its effect may be uncertain; inspect the page before retrying. No operation was replayed.")?;
        if !response.status().is_success() {
            return Err(format!(
                "Local Oya connection failed (HTTP {}). Check Oya’s connection and retry; Keel did not switch browsers.",
                response.status().as_u16()
            ));
        }
        if let Some(session) = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|v| v.to_str().ok())
        {
            if session.is_empty()
                || session.len() > 64
                || !session
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err("Invalid Oya session identity.".into());
            }
            if self.session.as_deref().is_some_and(|old| old != session) {
                return Err("Oya changed the session identity; owned tab references were invalidated. Retry the connection explicitly.".into());
            }
            self.session = Some(session.into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Local Oya disconnected during the operation; its effect is uncertain.")?
        {
            if bytes.len() + chunk.len() > RESPONSE_BYTES {
                return Err("Oya response exceeded its display limit.".into());
            }
            bytes.extend_from_slice(&chunk);
            if let Some(message) = message(&bytes, id) {
                return message;
            }
        }
        message(&bytes, id).ok_or("Oya returned no matching protocol response.".to_string())?
    }
}
fn message(bytes: &[u8], id: u64) -> Option<Result<Value, String>> {
    let parsed = serde_json::from_slice::<Value>(bytes).ok().or_else(|| {
        std::str::from_utf8(bytes)
            .ok()?
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|s| serde_json::from_str::<Value>(s.trim()).ok())
            .find(|v| v["id"].as_u64() == Some(id))
    })?;
    if parsed["id"].as_u64() != Some(id) {
        return None;
    }
    Some(if parsed.get("error").is_some() {
        Err("Oya rejected the browser protocol request.".into())
    } else {
        Ok(parsed["result"].clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn installation() -> Installation {
        Installation {
            application: "/Oya.app".into(),
            executable: "/Oya.app/Oya".into(),
        }
    }
    #[test]
    fn connection_requires_a_loopback_endpoint_bearer_and_verified_listener() {
        let valid = json!({"url":"http://127.0.0.1:47321/mcp","headers":{"Authorization":"Bearer fixture"}});
        assert!(
            candidate_with(&valid, &installation(), |port, _| if port == 47321 {
                Ok(9)
            } else {
                Err("wrong listener".into())
            })
            .is_ok()
        );
        assert!(
            candidate_with(&valid, &installation(), |_, _| Err("wrong listener".into())).is_err()
        );
        for bad in [
            json!({"url":"http://example.com/mcp","headers":{"Authorization":"Bearer fixture"}}),
            json!({"url":"http://127.0.0.1:47321/mcp?token=bad","headers":{"Authorization":"Bearer fixture"}}),
            json!({"url":"http://127.0.0.1:47321/mcp","headers":{"Authorization":"Bearer x\ny"}}),
        ] {
            assert!(
                candidate_with(&bad, &installation(), |_, _| panic!(
                    "invalid input reached listener verification"
                ))
                .is_err()
            );
        }
    }
    #[test]
    fn protocol_parser_ignores_other_request_ids_and_accepts_json_or_event_streams() {
        assert!(message(br#"{"id":1,"result":{}}"#, 2).is_none());
        assert_eq!(
            message(br#"{"id":2,"result":{"ok":true}}"#, 2)
                .unwrap()
                .unwrap()["ok"],
            true
        );
        assert_eq!(
            message(b"data: {\"id\":2,\"result\":{\"ok\":true}}\n\n", 2)
                .unwrap()
                .unwrap()["ok"],
            true
        );
    }
}
