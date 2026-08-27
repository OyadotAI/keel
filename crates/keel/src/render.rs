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
