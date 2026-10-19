//! Keel's team in a project, for every coding agent on this machine: which files are Keel's own
//! text, which are stale, and the one-click update that brings them level.
//!
//! "Keel's own" is decided by content against `keel_generator::team::SHIPPED`, every version of
//! every team file Keel ever wrote — not by a record on this machine, which a repository adopted
//! before the record existed, or cloned by a teammate, would not have. A file whose bytes match a
//! shipped version and not the current one is stale and safe to replace; any other bytes are the
//! person's, and are left alone and said so.

use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode};
use camino::Utf8Path;
use keel_generator::team::{self as tmpl, Agent};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::serve::AppState;

/// The coding agents to install for: whichever of Antigravity and Codex are on this machine, and
/// Claude Code always — Keel's lanes, its scanner and its readiness all read `.claude/`.
pub fn installed() -> Vec<Agent> {
    let mut out = vec![Agent::Claude];
    if crate::permissions::usable_on_path("agy") {
        out.push(Agent::Antigravity);
    }
    if crate::permissions::usable_on_path("codex") {
        out.push(Agent::Codex);
    }
    out
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Whether `bytes` are a version of the team file at `rel` that some Keel shipped. Line endings
/// count as `\n`: a Windows checkout with `core.autocrlf` holds every file in CRLF, and is no
/// less Keel's for it.
pub fn shipped(rel: &str, bytes: &[u8]) -> bool {
    tmpl::id(rel).is_some_and(|id| listed(&id, &sha(&lf(bytes))))
}

fn lf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for (i, b) in bytes.iter().enumerate() {
        if !(*b == b'\r' && bytes.get(i + 1) == Some(&b'\n')) {
            out.push(*b);
        }
    }
    out
}

fn listed(id: &str, digest: &str) -> bool {
    tmpl::SHIPPED
        .lines()
        .any(|l| l.split_once(' ') == Some((digest, id)))
}

/// The two sections Keel writes into an instructions file, as `(SHIPPED id, heading, current)`.
const SECTIONS: &[(&str, &str, &str)] = &[
    (
        "section:lifecycle",
        tmpl::LIFECYCLE_HEADING,
        tmpl::LIFECYCLE,
    ),
    ("section:rules", tmpl::RULES_HEADING, tmpl::RULES),
];

fn normal(text: &str) -> String {
    text.replace("\r\n", "\n").trim_end().to_string()
}

/// The byte range of the section opening with `heading`: from the heading to the next `## `
/// heading or the end. `None` when the file does not have it.
fn section_span(text: &str, heading: &str) -> Option<std::ops::Range<usize>> {
    let start = if text.starts_with(heading) {
        0
    } else {
        text.find(&format!("\n{heading}"))? + 1
    };
    let after = start + heading.len();
    let end = text[after..]
        .find("\n## ")
        .map_or(text.len(), |i| after + i + 1);
    Some(start..end)
}

/// The instructions file's Keel sections replaced by today's where they are still a version
/// Keel wrote, in the file's own line endings. `None` when there is nothing to replace.
fn refreshed_sections(text: &str) -> Option<String> {
    let crlf = text.contains("\r\n");
    let mut out = text.replace("\r\n", "\n");
    let mut changed = false;
    for (id, heading, current) in SECTIONS {
        let Some(span) = section_span(&out, heading) else {
            continue;
        };
        let have = normal(&out[span.clone()]);
        if have == normal(current) || !listed(id, &sha(have.as_bytes())) {
            continue;
        }
        let tail = &out[span.clone()][have.len()..];
        let tail = if tail.trim().is_empty() { tail } else { "\n" };
        out.replace_range(span, &format!("{}{tail}", normal(current)));
        changed = true;
    }
    changed.then(|| if crlf { out.replace('\n', "\r\n") } else { out })
}

/// What an update would do, path by path.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Plan {
    /// Team files not in the checkout and never in git: written new.
    pub add: Vec<String>,
    /// Keel's own text, an older version: replaced.
    pub replace: Vec<String>,
    /// Anything else at a team path — edited, a link, a folder: left alone.
    pub kept: Vec<String>,
    /// In git but gone from the checkout: the person deleted it, and it stays deleted.
    pub not_restored: Vec<String>,
    /// Whether this project has Keel's team at all. A project with none is the scanner's
    /// `agent/no-reviewers`, and is not an update.
    pub present: bool,
    pub agents: Agents,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Agents {
    pub claude: bool,
    pub antigravity: bool,
    pub codex: bool,
}

/// The team files for `agents`, and the frontend skill when the project already has it — the
/// update mirrors what adoption decided, and does not re-run the scanner to decide it again.
fn wanted(root: &Utf8Path, agents: &[Agent]) -> Vec<(String, String)> {
    tmpl::files_for(agents, root.join(tmpl::FRONTEND_SKILL_PATH).is_file())
}

/// Every path git knows under the team folders — in the index, at `HEAD`, or deleted in some
/// commit: three git calls, not one per file. A deletion the person committed is as much their
/// decision as one still staged, and offering the file back as "added" would undo it.
fn tracked(root: &Utf8Path) -> std::collections::HashSet<String> {
    let dirs = [".claude", ".agents", ".codex"];
    let mut out = std::collections::HashSet::new();
    let ls: Vec<&str> = ["ls-files", "--"].into_iter().chain(dirs).collect();
    let tree: Vec<&str> = ["ls-tree", "-r", "--name-only", "HEAD", "--"]
        .into_iter()
        .chain(dirs)
        .collect();
    let deleted: Vec<&str> = ["log", "--diff-filter=D", "--name-only", "--format=", "--"]
        .into_iter()
        .chain(dirs)
        .collect();
    for args in [ls, tree, deleted] {
        if let Ok(text) = crate::git::run(root, &args) {
            out.extend(text.lines().map(str::to_string));
        }
    }
    out
}

pub fn plan(root: &Utf8Path, agents: &[Agent]) -> Plan {
    let known = tracked(root);
    let mut p = Plan {
        present: tmpl::agents()
            .iter()
            .any(|(rel, _)| root.join(rel).exists()),
        agents: Agents {
            claude: agents.contains(&Agent::Claude),
            antigravity: agents.contains(&Agent::Antigravity),
            codex: agents.contains(&Agent::Codex),
        },
        ..Plan::default()
    };
    for (rel, body) in wanted(root, agents) {
        let at = root.join(&rel);
        match std::fs::symlink_metadata(&at) {
            Err(_) if known.contains(&rel) => p.not_restored.push(rel),
            Err(_) => p.add.push(rel),
            Ok(m) if !m.is_file() => p.kept.push(rel),
            Ok(_) => match std::fs::read(&at) {
                Ok(b) if lf(&b) == body.as_bytes() => {}
                Ok(b) if shipped(&rel, &b) => p.replace.push(rel),
                _ => p.kept.push(rel),
            },
        }
    }
    for (file, target) in crate::review::instruction_targets(root, agents) {
        let Ok((path, rel)) = target else {
            p.kept.push(file.into());
            continue;
        };
        if crate::writes::deleted_by_person(root, &rel) {
            p.not_restored.push(rel);
            continue;
        }
        match std::fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => p.add.push(rel),
            Err(_) => p.kept.push(rel),
            Ok(text) if refreshed_sections(&text).is_some() => p.replace.push(rel),
            Ok(text) if up_to_date(&text) => {}
            // Missing a section or the production line: adoption fills it, as it always has.
            Ok(_) => p.add.push(rel),
        }
    }
    p
}

/// Whether an instructions file already holds everything adoption appends.
fn up_to_date(text: &str) -> bool {
    let t = text.replace("\r\n", "\n");
    t.contains("docs/PRODUCTION.md") && tmpl::with_guidance(&t) == t
}

/// What `/api/team/update` did.
#[derive(Debug, Default, Serialize)]
pub struct Updated {
    pub replaced: Vec<String>,
    pub added: Vec<String>,
    pub kept: Vec<String>,
    pub not_restored: Vec<String>,
    /// Written, not committed: they held changes of the person's that were not committed yet.
    pub uncommitted: Vec<String>,
    pub committed: Option<String>,
    pub note: Option<String>,
}

pub async fn api_team(State(state): State<Arc<AppState>>) -> Json<Plan> {
    let root = state.repo();
    Json(crate::serve::blocking(move || plan(&root, &installed()), Plan::default()).await)
}

pub async fn api_update(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Updated>, crate::writes::Failure> {
    crate::serve::blocking(
        move || update(&state, &installed()),
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "the update was cancelled".into(),
        )),
    )
    .await
    .map(Json)
}

/// Always the project root, never a lane's checkout: a lane's tree is that lane's work.
fn update(state: &Arc<AppState>, agents: &[Agent]) -> Result<Updated, crate::writes::Failure> {
    let root = state.repo();
    let _held = crate::writes::hold(state, &root, "team")?;
    if let Some(op) = crate::writes::in_progress(&root) {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "Can't update the team while a {op} is in progress. Finish or abort it, then try \
                 again."
            ),
        ));
    }
    let p = plan(&root, agents);
    // Read before writing: a file holding anything of the person's that is not committed is
    // written but not committed, or Keel's commit would carry their work.
    let theirs: Vec<String> = p
        .replace
        .iter()
        .chain(p.add.iter())
        .filter(|rel| root.join(rel.as_str()).exists() && !crate::writes::clean_before(&root, rel))
        .cloned()
        .collect();
    let mut done = Updated {
        kept: p.kept,
        not_restored: p.not_restored,
        ..Updated::default()
    };
    // Exactly what the plan listed, and nothing it did not: the panel said "commits only these
    // files", and adoption's other files (the production checklist, lint rules) are adoption's.
    let current: std::collections::HashMap<String, String> =
        wanted(&root, agents).into_iter().collect();
    let files: std::collections::HashMap<String, &str> =
        crate::review::instruction_targets(&root, agents)
            .into_iter()
            .filter_map(|(file, t)| t.ok().map(|(_, rel)| (rel, file)))
            .collect();
    let failed = |rel: &str, e: String| (StatusCode::INTERNAL_SERVER_ERROR, format!("{rel}: {e}"));
    for rel in p.add {
        let wrote = match (files.get(&rel), current.get(&rel)) {
            (Some(file), _) => crate::review::append_guidance(&root, file)
                .map_err(|e| failed(&rel, e))?
                .is_some(),
            (None, Some(body)) => crate::writes::no_link_under(&root, &rel)
                .and_then(|_| crate::writes::create_new(&root.join(&rel), body.as_bytes()))
                .map_err(|e| failed(&rel, e))?,
            (None, None) => false,
        };
        if wrote {
            done.added.push(rel);
        }
    }
    for rel in p.replace {
        if replace_one(&root, &rel, current.get(&rel)).map_err(|e| failed(&rel, e))? {
            done.replaced.push(rel);
        }
    }
    let (ours, uncommitted): (Vec<String>, Vec<String>) = done
        .added
        .iter()
        .chain(done.replaced.iter())
        .cloned()
        .partition(|p| !theirs.contains(p));
    done.uncommitted = uncommitted;
    if !ours.is_empty() {
        let c = crate::writes::commit_only(&root, &ours, "Update the agent team");
        done.committed = c.sha;
        done.note = c.note;
    }
    Ok(done)
}

/// Replace one stale file or section, re-reading it first: under the hold, so the only window is
/// the person typing in the same millisecond. Returns whether anything was written.
// ponytail: read-then-rename race of milliseconds under the writer claim; a compare-and-swap
// needs a lock the person's editor does not take anyway.
fn replace_one(root: &Utf8Path, rel: &str, body: Option<&String>) -> Result<bool, String> {
    crate::writes::no_link_under(root, rel)?;
    let at = root.join(rel);
    if !std::fs::symlink_metadata(&at).is_ok_and(|m| m.is_file()) {
        return Ok(false);
    }
    let bytes = std::fs::read(&at).map_err(|e| e.to_string())?;
    let next = match body {
        Some(body) if shipped(rel, &bytes) && bytes != body.as_bytes() => body.clone(),
        Some(_) => return Ok(false),
        None => match String::from_utf8(bytes)
            .ok()
            .and_then(|t| refreshed_sections(&t))
        {
            Some(next) => next,
            None => return Ok(false),
        },
    };
    crate::writes::replace(at.as_std_path(), next.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[Agent] = &[Agent::Claude, Agent::Antigravity, Agent::Codex];

    /// Every file Keel writes today is in the shipped list, so the next Keel recognises it. A
    /// template or a renderer changed without this list is every adopter's file read as theirs.
    #[test]
    fn every_shipped_file_is_listed() {
        let mut missing = Vec::new();
        for (rel, body) in tmpl::files_for(ALL, true) {
            let id = tmpl::id(&rel).expect("a team path has an id");
            let line = format!("{} {id}", sha(body.as_bytes()));
            if !tmpl::SHIPPED.lines().any(|l| l == line) && !missing.contains(&line) {
                missing.push(line);
            }
        }
        for (id, _, current) in SECTIONS {
            let line = format!("{} {id}", sha(normal(current).as_bytes()));
            if !tmpl::SHIPPED.lines().any(|l| l == line) {
                missing.push(line);
            }
        }
        assert!(
            missing.is_empty(),
            "append these to crates/keel-generator/src/team/shipped.txt (never remove a line):\n{}",
            missing.join("\n")
        );
    }

    fn repo() -> (tempfile::TempDir, camino::Utf8PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = camino::Utf8PathBuf::from_path_buf(d.path().into()).unwrap();
        crate::git::run(&root, &["init", "-q"]).unwrap();
        (d, root)
    }

    fn write(root: &Utf8Path, rel: &str, body: &str) {
        std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        std::fs::write(root.join(rel), body).unwrap();
    }

    /// An old Keel's untouched file is stale; an edited one is the person's; a deleted one stays
    /// deleted; a missing one is added.
    #[test]
    fn a_plan_tells_keels_text_from_the_persons() {
        let (_d, root) = repo();
        let old = old_version(".claude/agents/principal.md");
        write(&root, ".claude/agents/principal.md", &old);
        write(&root, ".claude/agents/qa.md", "mine");
        write(&root, ".claude/agents/pm.md", tmpl::PM_AGENT);
        crate::git::run(&root, &["add", "-A"]).unwrap();
        std::fs::remove_file(root.join(".claude/agents/pm.md")).unwrap();
        let p = plan(&root, &[Agent::Claude]);
        assert!(p.present);
        assert!(p.replace.contains(&".claude/agents/principal.md".into()));
        assert!(p.kept.contains(&".claude/agents/qa.md".into()));
        assert!(p.not_restored.contains(&".claude/agents/pm.md".into()));
        assert!(p.add.contains(&".claude/agents/designer.md".into()));
        assert!(!p.add.iter().any(|r| r.starts_with(".agents/")));
        let all = plan(&root, ALL);
        assert!(all.add.contains(&".agents/agents/pm.md".into()));
        assert!(all.add.contains(&".codex/agents/pm.toml".into()));
        assert!(all.add.contains(&"AGENTS.md".into()));
    }

    /// A section still in an old Keel's words is replaced in place; the person's lines around it
    /// and a section they rewrote are not touched.
    #[test]
    fn a_stale_section_is_replaced_and_an_edited_one_kept() {
        let old = old_section("section:lifecycle");
        let text = format!(
            "# mine\n\nhouse rules\n\n{old}\n\n{}\n\n## Mine\n\nkeep\n",
            tmpl::RULES.trim_end()
        );
        let next = refreshed_sections(&text).expect("stale");
        assert!(next.starts_with("# mine\n\nhouse rules\n\n"));
        assert!(next.contains(tmpl::LIFECYCLE.trim_end()));
        assert!(next.ends_with("## Mine\n\nkeep\n"));
        assert_eq!(refreshed_sections(&next), None);
        let edited = text.replace("Every request", "Each request");
        assert_eq!(refreshed_sections(&edited), None);
    }

    /// Keel's own agy agent is kept by quarantine in any clone; one changed byte and it is not.
    #[test]
    fn quarantine_keeps_only_keels_own_antigravity_agents() {
        let body = tmpl::antigravity_agent(tmpl::PM_AGENT);
        assert!(shipped(".agents/agents/pm.md", body.as_bytes()));
        assert!(!shipped(
            ".agents/agents/pm.md",
            format!("{body} ").as_bytes()
        ));
        assert!(!shipped(".agents/agents/pm.md", tmpl::PM_AGENT.as_bytes()));
    }

    /// An old adopter, updated: the stale file replaced, the edited one kept, the deleted one
    /// left deleted, and one commit holding only Keel's paths.
    #[test]
    fn an_update_commits_only_keels_files() {
        let (_d, root) = repo();
        for (k, v) in [("user.name", "t"), ("user.email", "t@t")] {
            crate::git::run(&root, &["config", k, v]).unwrap();
        }
        crate::review::adopt_for(&root, &[Agent::Claude]).unwrap();
        write(
            &root,
            ".claude/agents/principal.md",
            &old_version(".claude/agents/principal.md"),
        );
        write(&root, ".claude/agents/qa.md", "mine");
        write(&root, "notes.txt", "the person's");
        crate::git::run(&root, &["add", ".claude", "CLAUDE.md", "docs"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "adopted"]).unwrap();
        let state = Arc::new(AppState::new(root.clone()));
        let done = update(&state, &[Agent::Claude]).unwrap();
        assert_eq!(
            done.replaced,
            vec![".claude/agents/principal.md".to_string()]
        );
        assert!(done.kept.contains(&".claude/agents/qa.md".into()));
        assert!(done.committed.is_some(), "{done:?}");
        let shown =
            crate::git::run(&root, &["show", "--name-only", "--format=%s", "HEAD"]).unwrap();
        assert!(shown.starts_with("Update the agent team"));
        assert!(shown.contains(".claude/agents/principal.md"));
        assert!(
            !shown.contains("notes.txt") && !shown.contains("qa.md"),
            "{shown}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".claude/agents/principal.md")).unwrap(),
            tmpl::PRINCIPAL_AGENT
        );
        // Up to date now: nothing left to replace.
        assert!(plan(&root, &[Agent::Claude]).replace.is_empty());
    }

    fn committed_repo() -> (tempfile::TempDir, camino::Utf8PathBuf) {
        let (d, root) = repo();
        for (k, v) in [("user.name", "t"), ("user.email", "t@t")] {
            crate::git::run(&root, &["config", k, v]).unwrap();
        }
        (d, root)
    }

    fn commit_all(root: &Utf8Path) {
        crate::git::run(root, &["add", "-A"]).unwrap();
        crate::git::run(root, &["commit", "-qm", "x"]).unwrap();
    }

    /// The person's uncommitted edits to the file behind `CLAUDE.md` are never in Keel's commit,
    /// whether it is reached through a link or an `@AGENTS.md` import.
    #[cfg(unix)]
    #[test]
    fn an_update_never_commits_edits_behind_a_link_or_an_import() {
        for linked in [true, false] {
            let (_d, root) = committed_repo();
            write(&root, "AGENTS.md", "# rules\n");
            if linked {
                std::os::unix::fs::symlink("AGENTS.md", root.join("CLAUDE.md")).unwrap();
            } else {
                write(&root, "CLAUDE.md", "@AGENTS.md\n");
            }
            crate::review::adopt_for(&root, &[Agent::Claude]).unwrap();
            commit_all(&root);
            write(&root, "AGENTS.md", "# rules\n\nMY PRIVATE EDIT\n");
            let state = Arc::new(AppState::new(root.clone()));
            let done = update(&state, &[Agent::Claude]).unwrap();
            let head = crate::git::run(&root, &["show", "HEAD:AGENTS.md"]).unwrap();
            assert!(
                !head.contains("MY PRIVATE EDIT"),
                "linked={linked}: {done:?}"
            );
            // And it is settled: nothing left to offer that the update cannot do.
            let after = plan(&root, &[Agent::Claude]);
            assert!(
                !after
                    .add
                    .iter()
                    .chain(&after.replace)
                    .any(|r| r.ends_with(".md") && !r.contains('/')),
                "linked={linked}: {after:?}"
            );
        }
    }

    /// A deleted `CLAUDE.md` stays deleted and is not offered forever.
    #[test]
    fn a_deleted_instructions_file_is_not_restored() {
        let (_d, root) = committed_repo();
        crate::review::adopt_for(&root, &[Agent::Claude]).unwrap();
        commit_all(&root);
        std::fs::remove_file(root.join("CLAUDE.md")).unwrap();
        // A team file whose deletion was committed stays deleted too.
        crate::git::run(&root, &["rm", "-q", ".claude/agents/pm.md"]).unwrap();
        crate::git::run(&root, &["commit", "-qm", "no pm"]).unwrap();
        let p = plan(&root, &[Agent::Claude]);
        assert!(
            p.not_restored.contains(&".claude/agents/pm.md".into()),
            "{p:?}"
        );
        assert!(p.not_restored.contains(&"CLAUDE.md".into()), "{p:?}");
        assert!(!p.add.contains(&"CLAUDE.md".into()));
    }

    /// A Windows checkout is still Keel's: CRLF team files are recognised, and a stale section in
    /// a CRLF file is replaced in CRLF.
    #[test]
    fn crlf_is_still_keels() {
        let crlf = tmpl::PM_AGENT.replace('\n', "\r\n");
        assert!(shipped(".claude/agents/pm.md", crlf.as_bytes()));
        let agy = tmpl::antigravity_agent(tmpl::PM_AGENT).replace('\n', "\r\n");
        assert!(shipped(".agents/agents/pm.md", agy.as_bytes()));
        let old = old_section("section:lifecycle");
        let text = format!("# mine\n\n{old}\n\n## Mine\n").replace('\n', "\r\n");
        let next = refreshed_sections(&text).expect("stale");
        assert!(
            !next.replace("\r\n", "").contains('\n'),
            "a bare LF in a CRLF file"
        );
        assert!(next.ends_with("## Mine\r\n"));
    }

    /// The update writes what its panel listed and nothing else.
    #[test]
    fn an_update_commits_only_what_it_offered() {
        let (_d, root) = committed_repo();
        crate::review::adopt_for(&root, &[Agent::Claude]).unwrap();
        write(&root, "package.json", "{}");
        std::fs::remove_file(root.join("docs/PRODUCTION.md")).unwrap();
        commit_all(&root);
        write(
            &root,
            ".claude/agents/principal.md",
            &old_version(".claude/agents/principal.md"),
        );
        commit_all(&root);
        let state = Arc::new(AppState::new(root.clone()));
        let done = update(&state, &[Agent::Claude]).unwrap();
        assert_eq!(
            done.replaced,
            vec![".claude/agents/principal.md".to_string()]
        );
        assert!(!root.join("docs/PRODUCTION.md").exists());
        assert!(!root.join(tmpl::ESLINT_RULES_PATH).exists());
    }

    /// An older shipped `principal.md` and lifecycle section, kept as fixtures: CI checks out
    /// without history, so they cannot be read from git. Each must still be in the shipped list.
    fn old_version(rel: &str) -> String {
        assert_eq!(rel, ".claude/agents/principal.md");
        let old = include_str!("../tests/fixtures/team-old-principal.md").replace("\r\n", "\n");
        assert!(
            shipped(rel, old.as_bytes()),
            "the fixture is a shipped version"
        );
        assert_ne!(
            old,
            tmpl::PRINCIPAL_AGENT,
            "the fixture is an older version"
        );
        old
    }

    fn old_section(id: &str) -> String {
        assert_eq!(id, "section:lifecycle");
        let old = normal(include_str!("../tests/fixtures/team-old-lifecycle.md"));
        assert!(
            listed(id, &sha(old.as_bytes())),
            "the fixture is a shipped section"
        );
        assert_ne!(
            old,
            normal(tmpl::LIFECYCLE),
            "the fixture is an older section"
        );
        old
    }
}
