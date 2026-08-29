//! Rendering a scan report for a terminal.
//!
//! Kept free of colour codes and cursor control so the output is equally readable when piped into a
//! file or pasted into a chat — which is how the report is expected to travel.

use keel_scanner::{Report, Severity};
use std::fmt::Write as _;

pub fn report(report: &Report) -> String {
    let mut out = String::new();

    let verdict = if report.is_shippable() {
        "no blocking issues"
    } else {
        "not ready to deploy"
    };
    let _ = writeln!(out, "\nReadiness {}/100 — {verdict}\n", report.score);
    if let Some(p) = &report.profile {
        if p.template != "blank" {
            let _ = writeln!(
                out,
                "Looks like: {} (like {}) · {}% · from {}",
                p.template_title,
                p.like,
                p.confidence,
                p.signals.join(", ")
            );
        }
        if !p.hosting.is_empty() {
            let hosts: Vec<&str> = p.hosting.iter().map(|h| h.name()).collect();
            let _ = writeln!(out, "Runs on: {}", hosts.join(" + "));
        }
        if p.template != "blank" || !p.hosting.is_empty() {
            let _ = writeln!(out);
        }
    }

    if report.findings.is_empty() {
        let _ = writeln!(out, "  Nothing to report.\n");
        return out;
    }

    for (dimension, findings) in report.by_dimension() {
        let _ = writeln!(out, "{}", dimension.label());
        for finding in findings {
            let _ = writeln!(out, "  [{}] {}", label(finding.severity), finding.title);
            if let Some(path) = &finding.path {
                let _ = writeln!(out, "        {path}");
            }
            let _ = writeln!(out, "        fix: {}", finding.fix.description());
        }
        let _ = writeln!(out);
    }

    if !report.plan.is_empty() {
        let _ = writeln!(out, "The road to production, in order");
        for (i, phase) in report.plan.iter().enumerate() {
            let _ = writeln!(out, "  {}. {} — {}", i + 1, phase.title, phase.why);
            for id in &phase.findings {
                if let Some(f) = report.findings.iter().find(|f| f.id == *id) {
                    let _ = writeln!(out, "       · {}", f.title);
                }
            }
        }
        let _ = writeln!(out);
    }

    let automatic = report.automatic_fixes().count();
    if automatic > 0 {
        let _ = writeln!(
            out,
            "{automatic} of {} findings can be fixed automatically — run `keel fix`.\n",
            report.findings.len()
        );
    }

    out
}

fn label(severity: Severity) -> &'static str {
    match severity {
        Severity::Critical => "critical",
        Severity::High => "high    ",
        Severity::Medium => "medium  ",
        Severity::Low => "low     ",
        Severity::Info => "info    ",
    }
}

/// Render the full workspace inventory.
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
