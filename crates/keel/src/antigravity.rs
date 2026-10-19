//! Antigravity's gate: the `PreToolUse` hook a Keel-driven `agy` runs before every tool.
//!
//! Headless `agy` refuses every command on its own and no hook answer can lift that, so Keel runs
//! it with `--dangerously-skip-permissions` and this hook is then the *only* thing between the
//! agent and the machine. That turns the rule `keel approve` lives by upside down: Claude's hook
//! fails open because Claude's allowlist is still underneath it; nothing is underneath this one.
//! So every path here ends in exactly one printed decision, and every path that is not a decision
//! somebody made is `deny`. Measured against agy 1.4.2: empty stdout is *allow*, while a non-zero
//! exit, output that is not JSON, and a hook killed at its timeout all block the tool — so the
//! one thing this must never do is print nothing.
//!
//! The hooks file is Keel's and lives outside the repository. `agy --add-dir <dir>` loads a
//! `.agents/hooks.json` from `<dir>` for that one invocation, which is how a turn gets the gate
//! without Keel writing into the project or into `~/.gemini`.

use camino::Utf8PathBuf;
use serde_json::Value;

use crate::approve::{Decision, HookInput};

/// Allowed without asking. Reads anywhere — the same default agy has, and the same reason Claude's
/// `Read` is not hooked: asking about every one turns the loop into clicking — and the two
/// subagent tools, whose effects all arrive here on their own. Nothing here changes the machine.
pub const LOCAL_ALLOW: &[&str] = &[
    "view_file",
    "list_dir",
    "grep_search",
    "find_by_name",
    "command_status",
    "wait",
    "wait_5_seconds",
    "finish",
    "list_permissions",
    // A subagent changes nothing itself: every tool it calls passes this same hook, under its own
    // conversation (measured: its `run_command` was refused here). `send_message` is how it
    // reports back to the turn that started it.
    "send_message",
];

pub const UNREACHABLE: &str = "Keel's approval service is not reachable, so this call was refused \
    without asking the person. Do not retry it or work around it with another tool. Stop and tell \
    the person Keel could not be reached.";
pub const UNREADABLE: &str = "Keel could not read this tool call, so it was refused without asking \
    the person. Do not retry it or work around it with another tool. Stop and say which call was \
    refused.";
pub const UNANSWERED: &str = "The person did not answer the approval request in time, so this call \
    was refused. They did not say no. Stop, say what you were about to run, and wait for their next \
    message.";
pub const PLANNING: &str = "This lane is in plan mode, so files cannot be changed. Do not try \
    another way to write. Finish the plan in your reply; the person switches out of plan mode to \
    apply it.";
pub const QUESTION: &str = "Keel cannot show ask_question to the person. Ask your question in your \
    reply as plain text and end your turn — their next message is the answer.";
pub const UNSUPPORTED: &str = "Keel does not support this tool in Antigravity lanes yet, so it was \
    refused. Do not work around it with another tool. Do what you can with commands and file edits, \
    or tell the person what you needed it for.";
pub const BACKGROUND: &str = "Keel does not run background commands for Antigravity lanes yet, so \
    this was refused. Run it in the foreground if it finishes on its own. If it never exits (a dev \
    server, a watcher), do not start it: tell the person to start it themselves.";
pub const CONFIG: &str = "This file is configuration that other tools run (git, an agent's \
    settings, an editor's tasks), so Keel does not let Antigravity write it. Do not write it another \
    way. Tell the person what you wanted to change and they can make the edit themselves.";
pub const DEFINE_SUBAGENT: &str = "Keel does not let Antigravity lanes define new subagents, \
    because a definition is a prompt that runs later without anyone reviewing it. Do not create one \
    another way, including by writing an agent file. Use invoke_subagent with one of the \
    project's existing agents, or do the work yourself in this conversation.";
pub const WORKSPACE: &str = "Keel only runs Antigravity subagents in this turn's own workspace \
    (Workspace: inherit), because that is where its approvals reach them. Call invoke_subagent \
    again without a branch or shared workspace.";
pub const MCP_ARGS: &str = "This MCP call's arguments are too large, or not a JSON object, for the \
    person to read on an approval card, so Keel refused it. Call the tool with smaller, structured \
    arguments.";
pub const SED: &str = "Keel does not run sed scripts for Antigravity lanes, because a script can \
    write other files or run commands. Make the same change with replace_file_content.";
pub const LINK: &str = "This path goes through a symbolic link, so Keel does not let Antigravity \
    write it. Write the file at its real location instead.";
const NO_PATH: &str = "This file tool call names no file, so Keel could not ask about it and \
    refused it. Call it again with the file's path.";
const REFUSED: &str = "The person refused this call. Do not substitute another command for it.";

/// The largest call the hook reads. A `write_to_file` carries the whole file; past this it is
/// refused rather than half-read and misjudged.
const STDIN_CAP: u64 = 16 * 1024 * 1024;

#[derive(Debug, PartialEq)]
pub enum Route {
    Allow,
    /// A Claude-shaped tool and input for `approve::ask`, so the allowlist, trust, session rules
    /// and the approval card apply exactly as they do to Claude.
    Ask(String, Value),
    Deny(&'static str),
}

/// What happens to one agy tool call before anybody is asked. A tool not named here — including
/// one a later agy adds in a self-update — is refused: under skip-permissions an unknown tool is an
/// ungated one.
pub fn route(tool: &str, args: &Value, plan: bool) -> Route {
    if LOCAL_ALLOW.contains(&tool) {
        return Route::Allow;
    }
    let (claude, input) = keel_workspace::conversation::antigravity_tool(tool, args);
    match tool {
        // A shell can write, and a planning lane holds no writer claim.
        "run_command" if plan => Route::Deny(PLANNING),
        "run_command" if input["command"].as_str().is_none_or(str::is_empty) => {
            Route::Deny(UNREADABLE)
        }
        "write_to_file"
        | "replace_file_content"
        | "multi_replace_file_content"
        | "notebook_edit"
            if plan =>
        {
            Route::Deny(PLANNING)
        }
        "write_to_file"
        | "replace_file_content"
        | "multi_replace_file_content"
        | "notebook_edit" => {
            let path = input["file_path"]
                .as_str()
                .or(input["notebook_path"].as_str());
            match path {
                None | Some("") => Route::Deny(NO_PATH),
                Some(p) if runs_something(p) => Route::Deny(CONFIG),
                Some(p) if through_a_link(p) => Route::Deny(LINK),
                Some(_) => Route::Ask(claude, input),
            }
        }
        "run_command" | "search_web" | "read_url_content" => Route::Ask(claude, input),
        "ask_question" => Route::Deny(QUESTION),
        "define_subagent" => Route::Deny(DEFINE_SUBAGENT),
        // Measured only in the inherited workspace: its tools came back through this hook. A
        // `branch` or `share` workspace is another checkout, where nothing says the hook loads.
        "invoke_subagent" if inherits(args) => Route::Allow,
        "invoke_subagent" => Route::Deny(WORKSPACE),
        // An MCP tool's effects are unknown, so a planning lane does not call one.
        "call_mcp_tool" if plan => Route::Deny(PLANNING),
        "call_mcp_tool" => mcp_call(args),
        // A sed script is a program: `w` writes any file, `e` runs a command. Nothing here can
        // read one safely, and replace_file_content does the same edit as data.
        "sed_file" => Route::Deny(SED),
        _ => Route::Deny(UNSUPPORTED),
    }
}

/// A path whose content something runs later without asking: git's config and hooks, every
/// agent's settings, an editor's tasks. An edit inside the repository is otherwise allowed without
/// a card — for Claude because Claude Code checks it after Keel; for agy nothing does — so these
/// are refused outright, trusted project or not.
fn runs_something(path: &str) -> bool {
    const DIRS: &[&str] = &[
        ".git", ".claude", ".codex", ".agents", ".agent", "_agents", "_agent", ".vscode", ".idea",
        ".husky", ".keel", ".gemini",
    ];
    const FILES: &[&str] = &[".mcp.json", ".envrc", ".gitmodules"];
    // macOS and Windows fold case, and Windows drops trailing dots and spaces: `.GIT/config` is
    // `.git/config` there. And a link inside the repository can lead into `.git`, so the path is
    // checked again once the part of it that exists is resolved.
    let hit = |path: &str| {
        let parts: Vec<String> = path
            .split(['/', '\\'])
            .map(|p| p.trim_end_matches(['.', ' ']).to_ascii_lowercase())
            .collect();
        parts.iter().any(|p| DIRS.contains(&p.as_str()))
            || parts.last().is_some_and(|f| FILES.contains(&f.as_str()))
    };
    hit(path) || resolved(std::path::Path::new(path)).is_some_and(|r| hit(&r))
}

/// Whether writing `path` would follow a symbolic link: the file itself is one — a dangling link
/// to `.git/hooks/pre-commit` resolves to nothing, so no canonical path can catch it — or a
/// folder on the way is one that points into config. Writing through a link is never needed for
/// an edit, so it is refused rather than reasoned about.
fn through_a_link(path: &str) -> bool {
    let p = std::path::Path::new(path);
    if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
        return true;
    }
    p.ancestors().skip(1).any(|a| {
        std::fs::symlink_metadata(a).is_ok_and(|m| m.file_type().is_symlink())
            && std::fs::read_link(a).is_ok_and(|t| runs_something(&t.to_string_lossy()))
    })
}

/// The path with its deepest existing ancestor canonicalized and the rest appended.
fn resolved(path: &std::path::Path) -> Option<String> {
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(at) {
            let mut out = real;
            out.extend(rest.iter().rev());
            return out.to_str().map(str::to_string);
        }
        rest.push(at.file_name()?);
        at = at.parent()?;
    }
}

/// An MCP call as Claude names it, `mcp__<server>__<tool>`, so the allowlist, the person's deny
/// rules and the card treat it exactly as they treat Claude's. A name holding `__` could forge a
/// different split — `a__b` + `c` reading as server `a`, tool `b__c` — and slip past a rule.
fn mcp_call(args: &Value) -> Route {
    let part = |k: &str| {
        args[k].as_str().filter(|s| {
            !s.is_empty()
                && !s.contains("__")
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
    };
    match (part("ServerName"), part("ToolName")) {
        (Some(server), Some(tool)) => match args.get("Arguments") {
            None | Some(Value::Null) => {
                Route::Ask(format!("mcp__{server}__{tool}"), serde_json::json!({}))
            }
            Some(a) if a.is_object() && a.to_string().len() <= MCP_ARGS_CAP => {
                Route::Ask(format!("mcp__{server}__{tool}"), a.clone())
            }
            Some(_) => Route::Deny(MCP_ARGS),
        },
        _ => Route::Deny(UNREADABLE),
    }
}

/// Whether every subagent asked for runs in this turn's own workspace.
fn inherits(args: &Value) -> bool {
    // Fails closed: every entry an object, its workspace absent or exactly "inherit".
    args["Subagents"].as_array().is_some_and(|a| {
        a.iter().all(|s| {
            s.is_object()
                && match s.get("Workspace") {
                    None | Some(Value::Null) => true,
                    Some(w) => w.as_str() == Some("inherit"),
                }
        })
    })
}

/// Arguments larger than a card can honestly show, or not an object at all, are refused: the
/// person would be approving what they cannot read.
const MCP_ARGS_CAP: usize = 16 * 1024;

/// The refusal for a call one of the person's own Claude deny rules covers.
pub fn denied(rule: &str) -> String {
    format!(
        "The person's Claude Code settings deny this call (`{rule}` in ~/.claude/settings.json), so \
         Keel refused it without asking. Do not run it another way or with another tool. Say what \
         you needed it for; only the person can change that rule."
    )
}

/// `keel antigravity-hook`: read the call on stdin, print one decision, exit 0.
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
    println!("{}", output(&decision));
}

type Env<'a> = &'a dyn Fn(&str) -> Option<String>;
type Ask<'a> = &'a dyn Fn(u16, HookInput) -> Option<Decision>;

fn decide(raw: &str, env: Env, ask: Ask) -> Decision {
    let Ok(call) = serde_json::from_str::<Value>(raw) else {
        return deny(UNREADABLE);
    };
    let Some(tool) = call["toolCall"]["name"].as_str() else {
        return deny(UNREADABLE);
    };
    // No mode said is planning: the narrower of the two.
    let plan = env("KEEL_AGY_MODE").as_deref() != Some("acceptEdits");
    let (tool_name, tool_input) = match route(tool, &call["toolCall"]["args"], plan) {
        Route::Allow => return allow(),
        Route::Deny(reason) => return deny(reason),
        Route::Ask(t, i) => (t, i),
    };
    let Some(port) = env("KEEL_AGY_PORT").and_then(|p| p.parse::<u16>().ok()) else {
        return deny(UNREACHABLE);
    };
    let conversation = call["conversationId"].as_str().unwrap_or_default();
    // The tripwire matches on this, so a step it cannot name is not one it can vouch for.
    let Some(step) = call["stepIdx"].as_u64() else {
        return deny(UNREADABLE);
    };
    let hook = HookInput {
        cwd: tool_input["Cwd"].as_str().unwrap_or_default().to_string(),
        tool_name,
        tool_input,
        tool_use_id: format!("agy:{conversation}:{step}"),
        session_id: conversation.to_string(),
        lane: env("KEEL_AGY_LANE").unwrap_or_default(),
        provider: "antigravity".into(),
    };
    match ask(port, hook) {
        None => deny(UNREACHABLE),
        // `defer` is "Claude's allowlist decides" — here, the allowlist already did.
        Some(d) if d.decision == "allow" || d.decision == "defer" => allow(),
        Some(d) if d.decision == "deny" && !d.reason.is_empty() => Decision {
            decision: "deny".into(),
            reason: d.reason,
        },
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

fn output(d: &Decision) -> String {
    if d.reason.is_empty() {
        serde_json::json!({ "decision": d.decision })
    } else {
        serde_json::json!({ "decision": d.decision, "reason": d.reason })
    }
    .to_string()
}

/// The folder a turn passes with `--add-dir`, holding `.agents/hooks.json` pointed at this binary.
/// Rewritten only when it differs, through a rename, and never through a link: a turn that cannot
/// have this file does not start, because without it the turn has no gate at all.
pub fn hooks_dir(exe: &std::path::Path) -> Result<Utf8PathBuf, String> {
    let dir = crate::permissions::permissions_dir()?.with_file_name("antigravity");
    write_hooks(&dir, exe)?;
    Ok(dir)
}

fn write_hooks(dir: &camino::Utf8Path, exe: &std::path::Path) -> Result<(), String> {
    let file = dir.join(".agents").join("hooks.json");
    let exe = exe.to_str().ok_or("Keel's own path is not UTF-8")?;
    // `sh -c` on Unix; on Windows agy uses `cmd /c`, which takes double quotes.
    #[cfg(windows)]
    let exe = format!("\"{exe}\"");
    #[cfg(not(windows))]
    let exe = crate::permissions::sh_quote(exe);
    // Above `approve::WAIT` plus the client's own slack, or agy kills the hook — a block, so not
    // unsafe, but a refusal the person never got to answer.
    let timeout = crate::approve::WAIT.as_secs() + 30;
    let body = serde_json::to_vec_pretty(&serde_json::json!({
        "keel": { "PreToolUse": [{ "matcher": "*", "hooks": [{
            "type": "command",
            "command": format!("{exe} antigravity-hook"),
            "timeout": timeout,
        }]}]}
    }))
    .map_err(|e| e.to_string())?;
    for p in [dir.to_owned(), dir.join(".agents"), file.clone()] {
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "{p} is a link, so Keel will not write its hook through it"
            ));
        }
    }
    if std::fs::read(&file).ok().as_deref() != Some(body.as_slice()) {
        crate::writes::replace(file.as_std_path(), &body)?;
    }
    Ok(())
}

/// The calls `approve::ask` has seen from Antigravity, so a tool that ran without passing through
/// it can be noticed. agy updates itself; if one day it stops loading hooks from `--add-dir`, every
/// tool runs unrestricted and nothing else on screen would say so.
pub fn asked() -> &'static std::sync::Mutex<std::collections::VecDeque<String>> {
    static S: std::sync::OnceLock<std::sync::Mutex<std::collections::VecDeque<String>>> =
        std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

pub fn note_asked(id: &str) {
    use crate::lock::Locked;
    let mut s = asked().locked();
    if s.len() >= 1024 {
        s.pop_front();
    }
    s.push_back(id.to_string());
}

/// Whether a finished agy step went through the gate. Local reads never reach the daemon, and a
/// call the hook denied ends in ERROR, so only a step that *ran* is asked about.
pub fn gated(conversation: &str, step: u64, tool: &str) -> bool {
    use crate::lock::Locked;
    // `invoke_subagent` is allowed by the hook itself, without asking, when it inherits.
    LOCAL_ALLOW.contains(&tool)
        || tool == "invoke_subagent"
        || asked()
            .locked()
            .iter()
            .any(|id| *id == format!("agy:{conversation}:{step}"))
}

pub const BYPASSED: &str = "Antigravity ran a tool without passing through Keel's approval hook, \
    so the turn was stopped. Update Keel and Antigravity, then send again.";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(tool: &str, args: Value) -> String {
        json!({"conversationId": "c1", "stepIdx": 4, "toolCall": {"name": tool, "args": args}})
            .to_string()
    }

    fn env(mode: &'static str) -> impl Fn(&str) -> Option<String> {
        move |k| match k {
            "KEEL_AGY_PORT" => Some("7777".into()),
            "KEEL_AGY_LANE" => Some("lane-1".into()),
            "KEEL_AGY_MODE" => Some(mode.into()),
            _ => None,
        }
    }

    /// Every tool agy 1.4.2 offers has a route, and a tool nobody has seen yet is refused.
    #[test]
    fn every_tool_agy_offers_is_routed_and_new_ones_are_refused() {
        let init = include_str!("../../keel-workspace/tests/fixtures/agy-hook-denials.jsonl")
            .lines()
            .next()
            .unwrap();
        let tools: Value = serde_json::from_str(init).unwrap();
        let tools = tools["init"]["tools"].as_array().unwrap();
        assert!(tools.len() > 50, "the fixture's tool list went missing");
        for t in tools {
            let t = t.as_str().unwrap();
            let r = route(
                t,
                &json!({"CommandLine": "ls", "TargetFile": "/r/a"}),
                false,
            );
            if !LOCAL_ALLOW.contains(&t) {
                assert_ne!(r, Route::Allow, "{t} runs with nobody asked");
            }
        }
        assert_eq!(
            route("brand_new_tool", &json!({}), false),
            Route::Deny(UNSUPPORTED)
        );
    }

    /// A planning lane cannot write, by shell or by edit.
    #[test]
    fn a_planning_lane_writes_nothing() {
        assert_eq!(
            route("run_command", &json!({"CommandLine": "touch x"}), true),
            Route::Deny(PLANNING)
        );
        assert_eq!(
            route("write_to_file", &json!({"TargetFile": "/r/a"}), true),
            Route::Deny(PLANNING)
        );
        assert_eq!(route("view_file", &json!({}), true), Route::Allow);
    }

    /// Inside the repository is not the same as harmless: these run later, unasked.
    #[test]
    fn config_that_runs_things_is_never_written() {
        for p in [
            "/r/.git/config",
            "/r/.git/hooks/pre-commit",
            "/r/.claude/settings.json",
            "/r/.agents/hooks.json",
            "/r/sub/.vscode/tasks.json",
            "/r/.mcp.json",
            "/r/.envrc",
            "/r/.GIT/config",
            "/r/.Claude/settings.json",
            "/r/.git./config",
        ] {
            assert_eq!(
                route("write_to_file", &json!({"TargetFile": p}), false),
                Route::Deny(CONFIG),
                "{p}"
            );
        }
        assert!(matches!(
            route(
                "write_to_file",
                &json!({"TargetFile": "/r/src/git.rs"}),
                false
            ),
            Route::Ask(..)
        ));
    }

    /// A link inside the repository that leads into `.git` is `.git`.
    #[cfg(unix)]
    #[test]
    fn a_link_into_git_is_git() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".git/hooks")).unwrap();
        std::os::unix::fs::symlink(d.path().join(".git/hooks"), d.path().join("tools")).unwrap();
        let p = d.path().join("tools/pre-commit");
        assert!(runs_something(p.to_str().unwrap()));
        assert!(!runs_something(d.path().join("src/a.rs").to_str().unwrap()));
    }

    /// A dangling link into `.git` resolves to nothing, so it is caught as a link.
    #[cfg(unix)]
    #[test]
    fn a_write_through_a_link_is_refused() {
        let d = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(".git/hooks/pre-commit", d.path().join("notes.md")).unwrap();
        let p = d.path().join("notes.md");
        assert_eq!(
            route(
                "write_to_file",
                &json!({"TargetFile": p.to_str().unwrap()}),
                false
            ),
            Route::Deny(LINK)
        );
        assert_eq!(
            route("sed_file", &json!({"TargetFile": "/r/a.rs"}), false),
            Route::Deny(SED)
        );
    }

    /// The X2 shapes: an MCP call is asked about under Claude's name for it; a subagent runs, and
    /// its own calls come back through this hook; defining one stays refused.
    #[test]
    fn subagents_and_mcp_calls_route_as_measured() {
        let call = json!({"Arguments": {"text": "hello"}, "ServerName": "qa", "ToolName": "echo"});
        assert_eq!(
            route("call_mcp_tool", &call, false),
            Route::Ask("mcp__qa__echo".into(), json!({"text": "hello"}))
        );
        assert_eq!(route("call_mcp_tool", &call, true), Route::Deny(PLANNING));
        let forged = json!({"ServerName": "a__b", "ToolName": "c"});
        assert_eq!(
            route("call_mcp_tool", &forged, false),
            Route::Deny(UNREADABLE)
        );
        assert_eq!(
            route("call_mcp_tool", &json!({"ServerName": "qa"}), false),
            Route::Deny(UNREADABLE)
        );
        let sub = json!({"Subagents": [{"Model": "inherit", "Prompt": "x", "Role": "r"}]});
        assert_eq!(route("invoke_subagent", &sub, false), Route::Allow);
        let odd = json!({"Subagents": [{"Prompt": "x", "Workspace": {"Type": "branch"}}]});
        assert_eq!(
            route("invoke_subagent", &odd, false),
            Route::Deny(WORKSPACE)
        );
        let bare = json!({"Subagents": ["x"]});
        assert_eq!(
            route("invoke_subagent", &bare, false),
            Route::Deny(WORKSPACE)
        );
        let branch = json!({"Subagents": [{"Prompt": "x", "Workspace": "branch"}]});
        assert_eq!(
            route("invoke_subagent", &branch, false),
            Route::Deny(WORKSPACE)
        );
        let big =
            json!({"ServerName": "qa", "ToolName": "echo", "Arguments": {"a": "x".repeat(20_000)}});
        assert_eq!(route("call_mcp_tool", &big, false), Route::Deny(MCP_ARGS));
        let text = json!({"ServerName": "qa", "ToolName": "echo", "Arguments": "{\"a\":1}"});
        assert_eq!(route("call_mcp_tool", &text, false), Route::Deny(MCP_ARGS));
        assert_eq!(
            route("send_message", &json!({"Message": "done"}), true),
            Route::Allow
        );
        assert!(gated("p1", 6, "send_message"));
        assert_eq!(
            route("define_subagent", &json!({}), false),
            Route::Deny(DEFINE_SUBAGENT)
        );
    }

    #[test]
    fn a_step_with_no_index_is_refused() {
        let raw = json!({"conversationId": "c1", "toolCall": {"name": "run_command", "args": {"CommandLine": "ls"}}});
        let never = |_: u16, _: HookInput| -> Option<Decision> { panic!("asked") };
        assert_eq!(
            decide(&raw.to_string(), &env("acceptEdits"), &never).reason,
            UNREADABLE
        );
    }

    #[test]
    fn a_write_with_no_path_is_refused() {
        assert_eq!(
            route("write_to_file", &json!({"CodeContent": "x"}), false),
            Route::Deny(NO_PATH)
        );
    }

    /// The card asks about the Claude-shaped call, so the allowlist and the card read it as Bash.
    #[test]
    fn a_command_is_asked_as_bash_in_its_conversation() {
        let seen = std::cell::RefCell::new(None);
        let d = decide(
            &call("run_command", json!({"CommandLine": "ls -la", "Cwd": "/r"})),
            &env("acceptEdits"),
            &|port, hook| {
                assert_eq!(port, 7777);
                *seen.borrow_mut() = Some(hook);
                Some(Decision {
                    decision: "defer".into(),
                    reason: String::new(),
                })
            },
        );
        assert_eq!(d.decision, "allow");
        let hook = seen.into_inner().unwrap();
        assert_eq!(hook.tool_name, "Bash");
        assert_eq!(hook.tool_input["command"], "ls -la");
        assert_eq!(hook.session_id, "c1");
        assert_eq!(hook.tool_use_id, "agy:c1:4");
        assert_eq!(hook.lane, "lane-1");
        assert_eq!(hook.provider, "antigravity");
    }

    /// Every path that is not somebody's yes is a no — and never silence, which agy reads as yes.
    #[test]
    fn the_hook_fails_closed() {
        let never = |_: u16, _: HookInput| -> Option<Decision> { panic!("asked") };
        let down = |_: u16, _: HookInput| None;
        let cmd = call("run_command", json!({"CommandLine": "ls"}));
        assert_eq!(
            decide("not json", &env("acceptEdits"), &never).reason,
            UNREADABLE
        );
        assert_eq!(decide("{}", &env("acceptEdits"), &never).reason, UNREADABLE);
        assert_eq!(decide(&cmd, &env("acceptEdits"), &down).reason, UNREACHABLE);
        assert_eq!(decide(&cmd, &|_| None, &never).reason, PLANNING);
        let no_port = |k: &str| (k == "KEEL_AGY_MODE").then(|| "acceptEdits".to_string());
        assert_eq!(decide(&cmd, &no_port, &never).reason, UNREACHABLE);
        let said_no = |_: u16, _: HookInput| {
            Some(Decision {
                decision: "deny".into(),
                reason: "because".into(),
            })
        };
        assert_eq!(
            decide(&cmd, &env("acceptEdits"), &said_no).reason,
            "because"
        );
        let odd = |_: u16, _: HookInput| {
            Some(Decision {
                decision: "maybe".into(),
                reason: String::new(),
            })
        };
        assert_eq!(decide(&cmd, &env("acceptEdits"), &odd).decision, "deny");
        for d in [allow(), deny(UNREACHABLE)] {
            assert!(!output(&d).is_empty());
            assert!(serde_json::from_str::<Value>(&output(&d)).is_ok());
        }
    }

    /// The gate's file points at this binary with a timeout above Keel's own wait, follows this
    /// binary when it moves, and is never written through a link.
    #[test]
    fn the_hooks_file_is_keels_and_current() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = camino::Utf8Path::from_path(tmp.path()).unwrap().join("agy");
        write_hooks(&dir, std::path::Path::new("/opt/my keel/keel")).unwrap();
        let read = || -> Value {
            serde_json::from_slice(&std::fs::read(dir.join(".agents/hooks.json")).unwrap()).unwrap()
        };
        let hook = &read()["keel"]["PreToolUse"][0];
        assert_eq!(hook["matcher"], "*");
        assert_eq!(
            hook["hooks"][0]["command"],
            "'/opt/my keel/keel' antigravity-hook"
        );
        assert!(
            hook["hooks"][0]["timeout"].as_u64().unwrap() > crate::approve::WAIT.as_secs() + 20
        );
        write_hooks(&dir, std::path::Path::new("/elsewhere/keel")).unwrap();
        assert_eq!(
            read()["keel"]["PreToolUse"][0]["hooks"][0]["command"],
            "'/elsewhere/keel' antigravity-hook"
        );
        #[cfg(unix)]
        {
            let linked = camino::Utf8Path::from_path(tmp.path())
                .unwrap()
                .join("linked");
            std::os::unix::fs::symlink(tmp.path().join("agy"), &linked).unwrap();
            assert!(write_hooks(&linked, std::path::Path::new("/k")).is_err());
        }
    }

    #[test]
    fn a_tool_that_skipped_the_gate_is_noticed() {
        assert!(gated("c9", 1, "view_file"));
        assert!(!gated("c9", 2, "run_command"));
        note_asked("agy:c9:2");
        assert!(gated("c9", 2, "run_command"));
    }
}
