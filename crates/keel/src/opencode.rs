//! OpenCode's gate: the plugin a Keel-driven `opencode` loads, and the hook that plugin runs
//! before every tool.
//!
//! Measured on 1.18.30: `opencode run` refuses on its own anything it would have asked about, and
//! with its default permissions it asks about nothing at all. So Keel passes a plugin for that
//! one run (`OPENCODE_CONFIG_CONTENT`, which merges last over the person's config), and the plugin
//! does two things: it turns OpenCode's own "ask" into "allow" — keeping every "deny" the person
//! wrote — and it holds each tool in `tool.execute.before` until `keel opencode-hook` prints a
//! decision. A throw there becomes the tool's error text, and the model does not substitute.
//!
//! Like Antigravity's hook and unlike Claude's, nothing is underneath this one: every path ends
//! in one printed decision, and every path that is not a decision somebody made is `deny`. The
//! plugin reads anything but a printed `allow` as a refusal, so a hook that crashed refuses too.
//!
//! The plugin file is Keel's and lives outside the repository. `--pure` (which drops it) and
//! `--auto` are never passed.

use camino::Utf8PathBuf;
use serde_json::{Value, json};

use crate::antigravity::{
    MCP_ARGS, NO_PATH, PLANNING, REFUSED, Route, UNREACHABLE, UNREADABLE, runs_something,
    through_a_link,
};
use crate::approve::{Decision, HookInput};

/// Allowed without asking: reads, the model's own list, a skill's instructions, and `task` —
/// a subagent changes nothing itself, because every tool it calls passes this same hook under
/// its own session (measured). `invalid` is what OpenCode runs in place of a call it could not
/// parse; it returns the parse error and touches nothing.
pub const LOCAL_ALLOW: &[&str] = &[
    "read",
    "list",
    "grep",
    "glob",
    "todowrite",
    "todoread",
    "skill",
    "task",
    "invalid",
];

/// Tools that write a file at one named path.
const WRITES: &[&str] = &["write", "edit", "multiedit"];

pub const QUESTION: &str = "Keel shows questions through its own tool, not question. Call \
    keel_ask_user with your question and its options; it waits for the person's answer and \
    returns it.";
pub const BACKGROUND: &str = "Keel does not run background commands for OpenCode lanes yet, so \
    this was refused. Run it in the foreground if it finishes on its own. If it never exits (a dev \
    server, a watcher), do not start it: tell the person to start it themselves.";
const CONFIG: &str = "This file is configuration that other tools run (git, an agent's settings, \
    an editor's tasks), so Keel does not let OpenCode write it. Do not write it another way. Tell \
    the person what you wanted to change and they can make the edit themselves.";
const LINK: &str = "This path goes through a symbolic link, so Keel does not let OpenCode write \
    it. Write the file at its real location instead.";
pub const BYPASSED: &str = "OpenCode ran a tool without passing through Keel's approval hook, so \
    the turn was stopped. Update Keel and OpenCode, then send again.";

/// The largest call the hook reads; a `write` carries the whole file.
const STDIN_CAP: u64 = 16 * 1024 * 1024;
/// Arguments larger than a card can honestly show are refused.
const MCP_ARGS_CAP: usize = 16 * 1024;

/// The plugin. `lift` is the person's own permission config with every "ask" made "allow" — Keel
/// asks instead — and every "deny" kept. The object form only: a bare string resolved to nothing
/// usable when measured. The `event` hook is for a chat turn alone: a spent quota prints nothing
/// on `opencode run` and retries for ever, so a retry that names a limit ends the run with the
/// provider's words on stderr.
const PLUGIN: &str = r#"// Keel's approval gate for OpenCode. Written by Keel on every turn; edits are overwritten.
const env = process.env;
// ponytail: an `ask` anywhere under a tool's rules asks about every call of that tool; match the
// person's patterns here if one `git push*` rule makes every command a card.
const asking = (p) => (typeof p === "string" ? p === "ask" : Object.values(p ?? {}).some(asking));
const glob = (k) => new RegExp("^" + k.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".") + "$");
// Permissions that name no tool: OpenCode keeps deciding these itself, so they are never lifted.
const THEIRS = ["external_directory", "doom_loop"];
const lift = (p) =>
  typeof p === "string"
    ? p === "deny" ? "deny" : "allow"
    : Object.fromEntries(Object.entries(p ?? {}).map(([k, v]) => [k, lift(v)]));
export const KeelGate = async () => {
  let servers = [];
  let asks = [];
  return {
    config: async (cfg) => {
      servers = Object.keys(cfg.mcp ?? {});
      const own = cfg.permission;
      asks = typeof own === "string" ? (own === "ask" ? ["*"] : []) : Object.entries(own ?? {}).filter(([k, v]) => !THEIRS.includes(k) && asking(v)).map(([k]) => k);
      cfg.permission = typeof own === "string" ? { "*": lift(own) } : { "*": "allow", ...lift(own) };
      for (const k of THEIRS) if (own?.[k] !== undefined) cfg.permission[k] = own[k];
    },
    "tool.execute.before": async (input, output) => {
      let said = null;
      try {
        const key = ["write", "edit", "multiedit", "apply_patch"].includes(input.tool) ? "edit" : input.tool;
        const asked = asks.some((k) => glob(k).test(key));
        const call = { tool: input.tool, callID: input.callID, sessionID: input.sessionID, args: output.args, servers, asked };
        const p = Bun.spawn([env.KEEL_OC_EXE, "opencode-hook"], {
          stdin: new TextEncoder().encode(JSON.stringify(call)),
          stdout: "pipe",
          stderr: "ignore",
        });
        const text = await new Response(p.stdout).text();
        await p.exited;
        said = JSON.parse(text);
      } catch {}
      if (said?.decision !== "allow") throw new Error(said?.reason || "Keel could not decide this call, so it was refused without asking the person. Do not retry it or work around it with another tool. Stop and say which call was refused.");
    },
    event: async ({ event }) => {
      if (env.KEEL_OC_TERMINAL === "1" || event.type !== "session.status") return;
      const status = event.properties?.status;
      if (status?.type === "retry" && /quota|limit|429|exhausted|credit|billing/i.test(status.message ?? "")) {
        console.error(status.message);
        process.exit(75);
      }
    },
  };
};
"#;

/// What happens to one OpenCode tool call before anybody is asked. A tool with no counterpart —
/// `apply_patch` on some models, whatever a later OpenCode adds — is asked about under its own
/// name: under an allow-everything permission an unknown tool is an ungated one, so it is never
/// waved through.
pub fn route(tool: &str, args: &Value, servers: &[&str], plan: bool) -> Route {
    if LOCAL_ALLOW.contains(&tool) {
        return Route::Allow;
    }
    if tool == "question" {
        return Route::Deny(QUESTION);
    }
    let (claude, input) = keel_workspace::conversation::opencode_tool(tool, args, servers);
    if let Some(keel) = claude.strip_prefix("mcp__keel__") {
        // A question is not a permission: Keel's own card asks it, on any project, in any mode.
        if keel == crate::askmcp::TOOL {
            return Route::Allow;
        }
        // Keel's Browser tab writes nothing in the tree, so a planning lane may use it.
        if plan && !crate::browser::is_tool(keel) {
            return Route::Deny(PLANNING);
        }
    } else if plan && !matches!(tool, "webfetch" | "websearch") {
        // A shell can write, an unknown tool's effects are unknown, and a planning lane holds no
        // writer claim.
        return Route::Deny(PLANNING);
    }
    if claude.starts_with("mcp__") {
        return if args.is_object() && args.to_string().len() <= MCP_ARGS_CAP {
            Route::Ask(claude, input)
        } else if args.is_null() {
            Route::Ask(claude, json!({}))
        } else {
            Route::Deny(MCP_ARGS)
        };
    }
    if tool == "bash" && input["command"].as_str().is_none_or(str::is_empty) {
        return Route::Deny(UNREADABLE);
    }
    if WRITES.contains(&tool) {
        return match input["file_path"].as_str() {
            None | Some("") => Route::Deny(NO_PATH),
            Some(p) if runs_something(p) => Route::Deny(CONFIG),
            Some(p) if through_a_link(p) => Route::Deny(LINK),
            Some(_) => Route::Ask(claude, input),
        };
    }
    if tool == "apply_patch" {
        let paths = patched(args);
        return match paths
            .iter()
            .find(|p| runs_something(p) || through_a_link(p))
        {
            _ if paths.is_empty() => Route::Deny(NO_PATH),
            Some(p) if runs_something(p) => Route::Deny(CONFIG),
            Some(_) => Route::Deny(LINK),
            None => Route::Ask(claude, input),
        };
    }
    Route::Ask(claude, input)
}

type Env<'a> = &'a dyn Fn(&str) -> Option<String>;
type Ask<'a> = &'a dyn Fn(u16, HookInput) -> Option<Decision>;

/// The id a call is asked about under, and found again by when it has run.
fn call_id(session: &str, call: &str) -> String {
    format!("oc:{session}:{call}")
}

fn decide(raw: &str, env: Env, ask: Ask) -> Decision {
    let Ok(call) = serde_json::from_str::<Value>(raw) else {
        return deny(UNREADABLE);
    };
    let (Some(tool), Some(id)) = (call["tool"].as_str(), call["callID"].as_str()) else {
        return deny(UNREADABLE);
    };
    let servers: Vec<&str> = call["servers"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    // No mode said is planning: the narrower of the two.
    let plan = env("KEEL_OC_MODE").as_deref() != Some("acceptEdits");
    let asked = call["asked"] == true;
    let (tool_name, tool_input) = match route(tool, &call["args"], &servers, plan) {
        // A tool Keel would wave through, which the person's own rules say to ask about.
        Route::Allow if asked => {
            keel_workspace::conversation::opencode_tool(tool, &call["args"], &servers)
        }
        Route::Allow => return allow(),
        Route::Deny(reason) => return deny(reason),
        Route::Ask(t, i) => (t, i),
    };
    let Some(port) = env("KEEL_OC_PORT").and_then(|p| p.parse::<u16>().ok()) else {
        return deny(UNREACHABLE);
    };
    let session = call["sessionID"].as_str().unwrap_or_default();
    let hook = HookInput {
        cwd: env("KEEL_OC_CWD").unwrap_or_default(),
        tool_name,
        tool_input,
        tool_use_id: call_id(session, id),
        session_id: session.to_string(),
        lane: env("KEEL_OC_LANE").unwrap_or_default(),
        provider: if env("KEEL_OC_TERMINAL").as_deref() == Some("1") {
            crate::approve::OPENCODE_TERMINAL
        } else {
            crate::approve::OPENCODE
        }
        .into(),
        asked,
    };
    match ask(port, hook) {
        None => deny(UNREACHABLE),
        // `defer` is "Claude's allowlist decides" — here, the allowlist already did.
        Some(d) if d.decision == "allow" || d.decision == "defer" => allow(),
        Some(d) if d.decision == "deny" && !d.reason.is_empty() => d,
        Some(_) => deny(REFUSED),
    }
}

fn allow() -> Decision {
    Decision {
        decision: "allow".into(),
        reason: String::new(),
    }
}

fn deny(reason: &str) -> Decision {
    Decision {
        decision: "deny".into(),
        reason: reason.into(),
    }
}

/// `keel opencode-hook`: read the call on stdin, print one decision, exit 0.
pub fn run() {
    let mut raw = String::new();
    let read = std::io::Read::read_to_string(
        &mut std::io::Read::take(std::io::stdin(), STDIN_CAP + 1),
        &mut raw,
    );
    let decision = if read.is_err() || raw.len() as u64 > STDIN_CAP {
        deny(UNREADABLE)
    } else {
        decide(&raw, &|k| std::env::var(k).ok(), &|port, hook| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()?
                .block_on(crate::approve::request(port, &hook))
        })
    };
    println!(
        "{}",
        json!({ "decision": decision.decision, "reason": decision.reason })
    );
}

/// Why OpenCode does not start in a project nobody has trusted that ships OpenCode config.
pub const UNTRUSTED_CONFIG: &str = "This project has its own OpenCode config (opencode.json or \
    .opencode/), and OpenCode runs the plugins in it as soon as it starts. Keel has not started \
    it. Read those files, then trust the project to run OpenCode here, or remove them.";

/// Whether OpenCode would find project config for `cwd`: it looks from the folder it runs in up
/// to the repository's root. Measured on 1.18.30, `OPENCODE_DISABLE_PROJECT_CONFIG` drops what
/// that config *says* (permissions, servers, commands) and still imports its plugin files, whose
/// top-level code then runs. So an untrusted project that has any is not started at all.
pub fn project_config(cwd: &camino::Utf8Path) -> bool {
    const NAMES: &[&str] = &["opencode.json", "opencode.jsonc", ".opencode"];
    for dir in cwd.ancestors() {
        if NAMES.iter().any(|n| dir.join(n).symlink_metadata().is_ok()) {
            return true;
        }
        if dir.join(".git").symlink_metadata().is_ok() {
            break;
        }
    }
    false
}

/// Whether an `apply_patch` writes outside the project: what `edit_is_inside` answers for a
/// `write`, asked of every file the patch names. Trust covers the repository and nothing else.
pub fn patch_leaves(repo: &camino::Utf8Path, cwd: &str, tool: &str, input: &Value) -> bool {
    tool == "apply_patch"
        && patched(input).iter().any(|p| {
            let path = std::path::Path::new(cwd).join(p);
            !crate::approve::edit_is_inside(repo, &json!({ "file_path": path }))
        })
}

/// The files an `apply_patch` names. One approval must not cover a patch to a file that runs
/// later, so each is checked as a `write`'s path is; a patch naming none cannot be judged.
fn patched(args: &Value) -> Vec<&str> {
    const MARKS: &[&str] = &[
        // No space after the colon: OpenCode 1.18.30 reads `*** Add File:x` as a header too.
        "*** Add File:",
        "*** Update File:",
        "*** Delete File:",
        "*** Move to:",
    ];
    args.as_object()
        .into_iter()
        .flat_map(|o| o.values())
        .filter_map(Value::as_str)
        .flat_map(str::lines)
        .filter_map(|l| MARKS.iter().find_map(|m| l.trim().strip_prefix(m)))
        .map(str::trim)
        .collect()
}

/// What one run is given, and by whom.
pub struct Run<'a> {
    pub port: u16,
    pub lane: Option<&'a str>,
    /// The checkout, for an edit's "is this inside the project".
    pub cwd: &'a str,
    /// `acceptEdits`, or anything else for a planning turn.
    pub mode: &'a str,
    pub terminal: bool,
    /// An untrusted project's own `opencode.json` and `.opencode/` are not loaded: a plugin there
    /// is code that runs on the machine of whoever opens the repository.
    pub trusted: bool,
}

/// The environment of one Keel-run `opencode`: the plugin (written first — a run without it has
/// no gate, so a failure here stops the turn), Keel's MCP server, and what the hook needs to ask.
pub fn env(run: &Run) -> Result<Vec<(&'static str, String)>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_str().ok_or("Keel's own path is not UTF-8")?;
    let plugin = plugin_file()?;
    let config = json!({
        "mcp": { crate::askmcp::SERVER: {
            "type": "remote",
            "url": crate::askmcp::url(run.port, run.lane),
            "enabled": true,
        }},
        "plugin": [file_url(plugin.as_str())],
        // A question asked through `ask_user` waits for the person, not for OpenCode's 30 s.
        "experimental": { "mcp_timeout": crate::askmcp::tool_timeout().parse::<u64>().unwrap_or(0) },
    });
    let mut env = vec![
        ("OPENCODE_CONFIG_CONTENT", config.to_string()),
        ("KEEL_OC_EXE", exe.to_string()),
        ("KEEL_OC_PORT", run.port.to_string()),
        ("KEEL_OC_LANE", run.lane.unwrap_or("").to_string()),
        ("KEEL_OC_CWD", run.cwd.to_string()),
        ("KEEL_OC_MODE", run.mode.to_string()),
        // An update mid-turn would swap the binary under a format this file pins by measurement.
        ("OPENCODE_DISABLE_AUTOUPDATE", "1".to_string()),
    ];
    if run.terminal {
        env.push(("KEEL_OC_TERMINAL", "1".into()));
    }
    if !run.trusted {
        env.push(("OPENCODE_DISABLE_PROJECT_CONFIG", "1".into()));
    }
    Ok(env)
}

/// A path as the `file://` spec OpenCode's plugin list takes.
/// ponytail: the Windows form (`file:///C:/…`) follows the URL rule and is not measured there.
fn file_url(path: &str) -> String {
    let path = path.replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

/// Keel's plugin, in a folder of Keel's own. Rewritten only when it differs, through a rename,
/// and never through a link.
fn plugin_file() -> Result<Utf8PathBuf, String> {
    let dir = crate::permissions::permissions_dir()?.with_file_name("opencode");
    let file = dir.join("keel-gate.js");
    for p in [&dir, &file] {
        if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "{p} is a link, so Keel will not write its gate through it"
            ));
        }
    }
    if std::fs::read(&file).ok().as_deref() != Some(PLUGIN.as_bytes()) {
        crate::writes::replace(file.as_std_path(), PLUGIN.as_bytes())?;
    }
    Ok(file)
}

/// Whether a tool that ran went through the gate. Calls the hook allows on its own never reach
/// the daemon, and a call it refused ends in an error, so only one that *ran* is asked about.
/// OpenCode updates itself; if one day it stops loading the plugin, every tool runs unrestricted
/// and nothing else on screen would say so.
pub fn gated(session: &str, call: &str, tool: &str, input: &Value, plan: bool) -> bool {
    use crate::lock::Locked;
    route(tool, input, &[crate::askmcp::SERVER], plan) == Route::Allow
        || crate::antigravity::asked()
            .locked()
            .iter()
            .any(|id| *id == call_id(session, call))
}

/// The chat turns waiting to hear that a call asked to run, by lane: OpenCode's stream says a
/// tool once, when it has settled, so without this a five-minute `cargo test` has no row.
type Rows = std::collections::HashMap<String, (String, tokio::sync::mpsc::UnboundedSender<String>)>;

fn rows() -> &'static std::sync::Mutex<Rows> {
    static R: std::sync::OnceLock<std::sync::Mutex<Rows>> = std::sync::OnceLock::new();
    R.get_or_init(Default::default)
}

/// A chat turn's ear for its own calls; dropped with the turn, however the turn ends.
pub struct Listening(String);

impl Drop for Listening {
    fn drop(&mut self) {
        use crate::lock::Locked;
        rows().locked().remove(&self.0);
    }
}

/// Start hearing the calls `lane`'s turn asks about. `session` may be empty until it is known.
pub fn listen(
    lane: &str,
    session: &str,
) -> (Listening, tokio::sync::mpsc::UnboundedReceiver<String>) {
    use crate::lock::Locked;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    rows()
        .locked()
        .insert(lane.to_string(), (session.to_string(), tx));
    (Listening(lane.to_string()), rx)
}

/// The session a fresh run turned out to be.
pub fn heard_as(lane: &str, session: &str) {
    use crate::lock::Locked;
    if let Some(entry) = rows().locked().get_mut(lane) {
        entry.0 = session.to_string();
    }
}

/// A call asked to run: open its row in the turn that owns it. A subagent's call is another
/// session's — its end never reaches this stream, so its row would never close — and is left out.
pub fn announce(hook: &HookInput) {
    use crate::lock::Locked;
    let Some(call) = hook.tool_use_id.rsplit(':').next() else {
        return;
    };
    if let Some((session, tx)) = rows().locked().get(&hook.lane)
        && *session == hook.session_id
    {
        let _ = tx.send(
            json!({
                "type": "tool_start",
                "sessionID": hook.session_id,
                "part": {
                    "callID": call,
                    "tool": hook.tool_name,
                    "state": { "input": hook.tool_input },
                },
            })
            .to_string(),
        );
    }
}

/// `opencode models` as the composer's list: one `provider/model` a line.
pub fn parse_models(text: &str) -> Vec<Value> {
    text.lines()
        .map(str::trim)
        .filter(|l| model_arg(Some(l)).is_some())
        .map(|id| json!({ "value": id, "displayName": id }))
        .collect()
}

/// The `--model` argument for OpenCode, if there is one: `provider/model`, where a model may
/// carry a tag (`ollama/qwen2.5-coder:7b`). Anything else on that argument is not a model.
pub fn model_arg(model: Option<&str>) -> Option<&str> {
    model.filter(|m| {
        m.contains('/')
            && !m.starts_with('-')
            && m.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-._/:@".contains(c))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(tool: &str, args: Value) -> String {
        json!({"tool": tool, "callID": "c1", "sessionID": "ses_1", "args": args, "servers": ["keel", "oya"]})
            .to_string()
    }

    fn vars(mode: &'static str) -> impl Fn(&str) -> Option<String> {
        move |k| match k {
            "KEEL_OC_PORT" => Some("7777".into()),
            "KEEL_OC_LANE" => Some("lane-1".into()),
            "KEEL_OC_MODE" => Some(mode.into()),
            "KEEL_OC_CWD" => Some("/work".into()),
            _ => None,
        }
    }

    fn yes(_: u16, _: HookInput) -> Option<Decision> {
        Some(allow())
    }

    #[test]
    fn opencode_is_never_run_pure_or_auto() {
        let source = include_str!("agent.rs");
        let arm = source
            .split("fn opencode_command")
            .nth(1)
            .expect("the OpenCode arm")
            .split("\n}\n")
            .next()
            .unwrap();
        assert!(arm.contains("\"run\""));
        assert!(!arm.contains("--pure") && !arm.contains("--auto"));
    }

    #[test]
    fn opencode_reads_run_without_asking_and_effects_ask_as_claudes_tools() {
        for tool in LOCAL_ALLOW {
            assert_eq!(route(tool, &json!({}), &["keel"], true), Route::Allow);
        }
        let asked = std::cell::RefCell::new(None);
        let d = decide(
            &call("bash", json!({"command": "cargo test"})),
            &vars("acceptEdits"),
            &|port, hook| {
                *asked.borrow_mut() = Some((port, hook));
                Some(allow())
            },
        );
        assert_eq!(d.decision, "allow");
        let (port, hook) = asked.into_inner().unwrap();
        assert_eq!(port, 7777);
        assert_eq!(
            (hook.tool_name.as_str(), hook.provider.as_str()),
            ("Bash", "opencode")
        );
        assert_eq!(hook.tool_use_id, "oc:ses_1:c1");
        assert_eq!((hook.lane.as_str(), hook.cwd.as_str()), ("lane-1", "/work"));
        assert_eq!(
            route("edit", &json!({"filePath": "/work/a.rs"}), &[], false),
            Route::Ask(
                "Edit".into(),
                json!({"filePath": "/work/a.rs", "file_path": "/work/a.rs"})
            )
        );
        assert_eq!(
            route("oya_click", &json!({"id": 1}), &["keel", "oya"], false),
            Route::Ask("mcp__oya__click".into(), json!({"id": 1}))
        );
    }

    /// GPT models are given `apply_patch`; a later OpenCode will add tools nobody has seen.
    #[test]
    fn an_unknown_opencode_tool_is_asked_about() {
        assert_eq!(
            route("frobnicate", &json!({"x": 1}), &["keel"], false),
            Route::Ask("frobnicate".into(), json!({"x": 1}))
        );
        let patch = |file: &str| json!({"patchText": format!("*** Begin Patch\n*** Update File: {file}\n@@\n-a\n+b\n*** End Patch")});
        assert!(matches!(
            route("apply_patch", &patch("src/a.rs"), &["keel"], false),
            Route::Ask(..)
        ));
        assert_eq!(
            route("apply_patch", &patch(".git/hooks/pre-commit"), &[], false),
            Route::Deny(CONFIG)
        );
        assert_eq!(
            route("apply_patch", &json!({"patchText": "x"}), &[], false),
            Route::Deny(NO_PATH)
        );
        assert_eq!(
            route("apply_patch", &json!({}), &["keel"], true),
            Route::Deny(PLANNING)
        );
    }

    #[test]
    fn an_opencode_planning_lane_writes_nothing() {
        for (tool, args) in [
            ("bash", json!({"command": "ls"})),
            ("write", json!({"filePath": "/work/a"})),
            ("oya_click", json!({})),
        ] {
            let d = decide(&call(tool, args), &vars("plan"), &yes);
            assert_eq!((d.decision.as_str(), d.reason.as_str()), ("deny", PLANNING));
        }
        // Looking something up, a question and Keel's Browser tab are not writes.
        assert!(matches!(
            route("webfetch", &json!({"url": "https://x"}), &["keel"], true),
            Route::Ask(..)
        ));
        assert_eq!(
            route("keel_ask_user", &json!({}), &["keel"], true),
            Route::Allow
        );
        assert!(matches!(
            route("keel_browser_read", &json!({}), &["keel"], true),
            Route::Ask(..)
        ));
    }

    #[test]
    fn opencode_cannot_write_what_runs_later_or_ask_its_own_question() {
        for path in [
            "/work/.git/hooks/pre-commit",
            "/work/opencode.json",
            "/work/.opencode/plugin/x.js",
            "/work/.claude/settings.json",
        ] {
            assert_eq!(
                route("write", &json!({"filePath": path}), &[], false),
                Route::Deny(CONFIG),
                "{path}"
            );
        }
        assert_eq!(route("write", &json!({}), &[], false), Route::Deny(NO_PATH));
        assert_eq!(
            route("question", &json!({}), &[], false),
            Route::Deny(QUESTION)
        );
        assert_eq!(
            route("bash", &json!({"command": ""}), &[], false),
            Route::Deny(UNREADABLE)
        );
    }

    /// Nothing is underneath this hook, so everything that is not a yes is a no.
    #[test]
    fn an_opencode_call_nobody_decided_is_refused() {
        let bash = call("bash", json!({"command": "ls"}));
        let edits = vars("acceptEdits");
        assert_eq!(decide("not json", &edits, &yes).reason, UNREADABLE);
        assert_eq!(decide(&bash, &|_| None, &yes).reason, PLANNING);
        let no_port = |k: &str| (k == "KEEL_OC_MODE").then(|| "acceptEdits".to_string());
        assert_eq!(decide(&bash, &no_port, &yes).reason, UNREACHABLE);
        assert_eq!(decide(&bash, &edits, &|_, _| None).reason, UNREACHABLE);
        let silent = |_: u16, _: HookInput| Some(deny(""));
        assert_eq!(decide(&bash, &edits, &silent).reason, REFUSED);
        let ask = |_: u16, _: HookInput| {
            Some(Decision {
                decision: "ask".into(),
                reason: String::new(),
            })
        };
        assert_eq!(decide(&bash, &edits, &ask).decision, "deny");
    }

    #[test]
    fn an_opencode_tool_that_never_asked_stops_the_turn() {
        let bash = json!({"command": "ls"});
        assert!(!gated("ses_9", "c1", "bash", &bash, false));
        crate::antigravity::note_asked("oc:ses_9:c1");
        assert!(gated("ses_9", "c1", "bash", &bash, false));
        assert!(!gated("ses_9", "c2", "bash", &bash, false));
        // What the hook allows on its own never asked, and is not a bypass.
        assert!(gated("ses_9", "c3", "read", &json!({}), false));
        assert!(gated("ses_9", "c4", "keel_ask_user", &json!({}), false));
    }

    /// The gate keeps what the person denied and asks in place of OpenCode's own question.
    #[test]
    fn the_opencode_plugin_keeps_denies_and_fails_closed() {
        assert!(PLUGIN.contains(r#"p === "deny" ? "deny" : "allow""#));
        assert!(PLUGIN.contains(r#"said?.decision !== "allow") throw"#));
        assert!(PLUGIN.contains(r#""opencode-hook""#));
    }

    #[test]
    fn an_untrusted_project_runs_opencode_without_its_config() {
        let run = |trusted| Run {
            port: 7777,
            lane: Some("lane-1"),
            cwd: "/work",
            mode: "plan",
            terminal: false,
            trusted,
        };
        let has = |env: &[(&str, String)], k: &str| env.iter().any(|(name, _)| *name == k);
        let untrusted = env(&run(false)).unwrap();
        assert!(has(&untrusted, "OPENCODE_DISABLE_PROJECT_CONFIG"));
        assert!(!has(
            &env(&run(true)).unwrap(),
            "OPENCODE_DISABLE_PROJECT_CONFIG"
        ));
        let config: Value = serde_json::from_str(
            &untrusted
                .iter()
                .find(|(k, _)| *k == "OPENCODE_CONFIG_CONTENT")
                .unwrap()
                .1,
        )
        .unwrap();
        assert!(
            config["mcp"]["keel"]["url"]
                .as_str()
                .unwrap()
                .contains("lane=lane-1")
        );
        let plugin = config["plugin"][0].as_str().unwrap();
        assert!(plugin.starts_with("file:///") && plugin.ends_with("/opencode/keel-gate.js"));
        assert_eq!(
            file_url(r"C:\Users\a\gate.js"),
            "file:///C:/Users/a/gate.js"
        );
    }

    #[test]
    fn an_untrusted_project_with_opencode_config_is_found_from_its_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let root = camino::Utf8Path::from_path(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        std::fs::create_dir_all(root.join("repo/app/src")).unwrap();
        assert!(!project_config(&root.join("repo/app/src")));
        // Above the repository is not the project's: OpenCode stops at the root, and so does this.
        std::fs::write(root.join("opencode.json"), "{}").unwrap();
        assert!(!project_config(&root.join("repo/app/src")));
        std::fs::create_dir_all(root.join("repo/app/.opencode")).unwrap();
        assert!(project_config(&root.join("repo/app/src")));
    }

    #[tokio::test]
    async fn an_opencode_call_opens_its_row_in_its_own_turn_only() {
        let hook = |session: &str| HookInput {
            asked: false,
            tool_name: "Bash".into(),
            tool_input: json!({"command": "cargo test"}),
            tool_use_id: call_id(session, "c7"),
            session_id: session.into(),
            lane: "rows-lane".into(),
            cwd: String::new(),
            provider: "opencode".into(),
        };
        let (listening, mut rx) = listen("rows-lane", "");
        announce(&hook("ses_a"));
        heard_as("rows-lane", "ses_a");
        announce(&hook("ses_child"));
        announce(&hook("ses_a"));
        let line: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(line["type"], "tool_start");
        assert_eq!(line["part"]["callID"], "c7");
        assert_eq!(line["part"]["state"]["input"]["command"], "cargo test");
        drop(listening);
        assert!(rx.recv().await.is_none(), "one row, and the ear closes");
    }

    /// Trust is about the repository: a patch naming a file outside it is a question.
    #[test]
    fn an_opencode_patch_outside_the_project_is_not_covered_by_trust() {
        let dir = tempfile::tempdir().unwrap();
        let root = camino::Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let patch = |p: &str| json!({"patchText": format!("*** Update File: {p}\n@@\n-a\n+b")});
        let leaves = |p: &str| patch_leaves(&root, root.as_str(), "apply_patch", &patch(p));
        assert!(!leaves("src/a.rs"));
        assert!(leaves("../elsewhere.rs"));
        assert!(leaves("/etc/hosts"));
        let tight = json!({"patchText": "*** Update File: a.rs\n*** Add File:/etc/hosts\n+x"});
        assert!(
            patch_leaves(&root, root.as_str(), "apply_patch", &tight),
            "no space after the colon"
        );
        assert!(!patch_leaves(&root, "", "Bash", &patch("/etc/hosts")));
        assert!(
            patch_leaves(&root, "", "apply_patch", &patch("a.rs")),
            "no checkout said"
        );
    }

    /// The person's own `ask` reaches the daemon, which never lets trust answer it.
    #[test]
    fn the_persons_opencode_ask_rules_are_still_asked() {
        assert!(PLUGIN.contains("asks.some((k) => glob(k).test(key))"));
        let raw = json!({"tool": "bash", "callID": "c", "args": {"command": "ls"}, "asked": true});
        let seen = |_: u16, h: HookInput| Some(deny(if h.asked { "asked" } else { "lost" }));
        assert_eq!(
            decide(&raw.to_string(), &vars("acceptEdits"), &seen).reason,
            "asked"
        );
        // A read is waved through by Keel, unless the person said to ask about reads.
        let read = json!({"tool": "read", "callID": "c", "args": {"filePath": "a"}, "asked": true});
        assert_eq!(
            decide(&read.to_string(), &vars("acceptEdits"), &seen).reason,
            "asked"
        );
        assert!(PLUGIN.contains(r#"const THEIRS = ["external_directory", "doom_loop"]"#));
    }

    #[test]
    fn opencode_models_are_read_from_its_listing() {
        let got = parse_models("opencode/big-pickle\nollama/qwen2.5-coder:7b\n--help\nnot a row\n");
        assert_eq!(got.len(), 2);
        assert_eq!(got[1]["value"], "ollama/qwen2.5-coder:7b");
        assert_eq!(model_arg(Some("-m/x")), None);
        assert_eq!(model_arg(Some("a/b;rm")), None);
    }
}
