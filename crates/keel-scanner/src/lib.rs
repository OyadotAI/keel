//! Repo readiness scanning.
//!
//! The scanner answers two questions about a repository: can an agent safely work in it, and can it
//! run on Cloudflare? It is deliberately independent of every other crate — no network, no agent, no
//! cloud credentials — so it can ship on its own and be tested against a corpus of real repos.

mod catalog;
mod checks;
mod context;
mod finding;
mod profile;
mod report;
#[cfg(test)]
pub(crate) mod testutil;

pub use checks::default_checks;
pub use context::RepoContext;
pub use finding::{Dimension, Finding, Fix, Severity};
pub use profile::{Hosting, Profile, detect, has_frontend};
pub use report::{CheckState, Phase, Report};

/// Run every default check against `ctx` and collect the results into a report.
pub fn scan(ctx: &RepoContext) -> Report {
    let mut findings: Vec<Finding> = Vec::new();
    let mut judged: Vec<&'static str> = Vec::new();
    for check in default_checks() {
        let (found, ids) = check.evaluate(ctx);
        findings.extend(found);
        judged.extend(ids);
    }

    // Stable ordering: worst first, then by check id, so two runs over an unchanged repo produce
    // byte-identical reports. The corpus tests depend on this.
    findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.id.cmp(b.id))
            .then_with(|| a.path.cmp(&b.path))
    });

    let mut report = Report::with_profile(findings, detect(ctx));
    report.checks = report::checks(&report.findings, &judged);
    report
}

/// A single readiness check.
///
/// Checks are pure functions of the repo context. They never mutate the repo and never reach the
/// network — that keeps `keel scan` safe to run against a repository nobody has reviewed yet.
pub trait Check: Send + Sync {
    fn id(&self) -> &'static str;
    fn dimension(&self) -> Dimension;
    fn run(&self, ctx: &RepoContext) -> Vec<Finding>;

    /// True for a check whose one id is judged on every repository, so passing it is worth
    /// showing. Most are not: telling a static site it passed "rate limiting" is a claim about
    /// something nobody looked at.
    fn always(&self) -> bool {
        false
    }

    /// The findings, and every id this check actually judged here — passed or not. A check whose
    /// ids apply only in some repositories overrides this and names them at the same `if` that
    /// decides it, so "was checked" cannot drift from the condition that checked it.
    fn evaluate(&self, ctx: &RepoContext) -> (Vec<Finding>, Vec<&'static str>) {
        let judged = if self.always() {
            vec![self.id()]
        } else {
            Vec::new()
        };
        (self.run(ctx), judged)
    }
}
