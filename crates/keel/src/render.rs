//! Rendering the workspace inventory for a terminal.
//!
//! Kept free of colour codes and cursor control so the output is equally readable when piped into a
//! file or pasted into a chat. This is what `keel workspace` prints, and it is the reason the
//! daemon's knowledge stays independently runnable rather than reachable only through the app.

use std::fmt::Write as _;

pub fn workspace(ws: &keel_workspace::Workspace) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "\n{}\n", ws.repo);

    if ws.is_empty() {
        let _ = writeln!(
            out,
            "  Claude Code has no recorded state for this repository yet.\n"
        );
        return out;
    }

    let _ = write!(out, "{}", sessions(ws, false));

    section(&mut out, "Skills", ws.skills.len(), |out| {
        for skill in &ws.skills {
            let _ = writeln!(
                out,
                "  {:<28} {:<8} {}",
                keel_workspace::truncate(&skill.name, 27),
                skill.scope.label(),
                keel_workspace::truncate(skill.description.as_deref().unwrap_or(""), 60)
            );
        }
    });

    section(&mut out, "Plugins", ws.plugins.len(), |out| {
        for plugin in &ws.plugins {
            let _ = writeln!(
                out,
                "  {:<28} {:<8} {}",
                keel_workspace::truncate(&plugin.name, 27),
                plugin.scope.as_deref().unwrap_or("-"),
                plugin.marketplace.as_deref().unwrap_or("-")
            );
        }
    });

    section(&mut out, "Subagents", ws.agents.len(), |out| {
        for agent in &ws.agents {
            let _ = writeln!(
                out,
                "  {:<28} {:<8} {}",
                keel_workspace::truncate(&agent.name, 27),
                agent.scope.label(),
                keel_workspace::truncate(agent.description.as_deref().unwrap_or(""), 60)
            );
        }
    });

    // Hooks and MCP servers execute. They come last because they are what the reader should be
    // left looking at, and project-scoped entries are marked because those arrived with the
    // repository rather than from the user's own configuration.
    section(&mut out, "Hooks", ws.hooks.len(), |out| {
        for hook in &ws.hooks {
            let _ = writeln!(
                out,
                "  {} {:<20} {:<8} {}",
                if hook.is_untrusted() { "!" } else { " " },
                keel_workspace::truncate(&hook.event, 19),
                hook.scope.label(),
                keel_workspace::truncate(&hook.command, 54)
            );
        }
    });

    section(&mut out, "MCP servers", ws.mcp_servers.len(), |out| {
        for server in &ws.mcp_servers {
            let _ = writeln!(
                out,
                "  {} {:<20} {:<8} {}",
                if server.is_untrusted() { "!" } else { " " },
                keel_workspace::truncate(&server.name, 19),
                server.scope.label(),
                keel_workspace::truncate(server.endpoint.as_deref().unwrap_or(""), 54)
            );
        }
    });

    let untrusted = ws.untrusted_count();
    if untrusted > 0 {
        let _ = writeln!(
            out,
            "! {untrusted} item(s) came with this repository and will execute.\n  Review them, or run `keel trust` to quarantine them.\n"
        );
    }

    section(&mut out, "Commands", ws.commands.len(), |out| {
        for command in &ws.commands {
            let _ = writeln!(
                out,
                "  /{:<27} {}",
                keel_workspace::truncate(&command.name, 26),
                command.scope.label()
            );
        }
    });

    out
}

/// Render the session list. `all` shows every session rather than the recent window.
pub fn sessions(ws: &keel_workspace::Workspace, all: bool) -> String {
    const RECENT: usize = 10;
    let mut out = String::new();

    if ws.sessions.is_empty() {
        let _ = writeln!(out, "Sessions (0)\n");
        return out;
    }

    let shown = if all {
        ws.sessions.len()
    } else {
        ws.sessions.len().min(RECENT)
    };

    let _ = writeln!(out, "Sessions ({})", ws.sessions.len());
    for session in ws.sessions.iter().take(shown) {
        let _ = writeln!(
            out,
            "  {:<10} {:<44} {:>5} msg  {}",
            session.short_id(),
            keel_workspace::truncate(&session.display_title(), 43),
            session.messages,
            session
                .last_active
                .as_deref()
                .unwrap_or("")
                .replace('T', " ")
                .chars()
                .take(16)
                .collect::<String>(),
        );
    }
    if shown < ws.sessions.len() {
        let _ = writeln!(out, "  … {} older — pass --all", ws.sessions.len() - shown);
    }
    let _ = writeln!(out, "\n  resume: claude --resume <id>\n");

    out
}

/// Write a titled section, or a one-line note when it is empty.
fn section(out: &mut String, title: &str, count: usize, body: impl FnOnce(&mut String)) {
    let _ = writeln!(out, "{title} ({count})");
    if count == 0 {
        let _ = writeln!(out, "  none\n");
        return;
    }
    body(out);
    let _ = writeln!(out);
}
