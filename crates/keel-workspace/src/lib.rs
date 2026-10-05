//! Discovery of everything Claude Code knows about a project.
//!
//! Claude Code keeps a lot of state that has no interface: sessions live as JSONL transcripts under
//! `~/.claude/projects/`, skills and agents are Markdown files scattered across two scopes, plugins
//! are a JSON index, and hooks hide inside settings files. All of it is real, all of it changes how
//! an agent behaves in your repo, and none of it is visible while you work.
//!
//! This crate reads that state so Keel can show it.
//!
//! # Privacy
//!
//! Session transcripts contain the full text of everything discussed. [`discover_sessions`], which
//! runs constantly to populate lists, surfaces only titles, counts and timestamps — reading a
//! transcript to render a list is not licence to display its contents. [`tail`] is the separate,
//! explicit path for opening one session the user asked for by name, and it carries the id guard.

mod agents;
mod config;
pub mod conversation;
mod plugins;
mod sessions;
mod skills;

pub use agents::{Agent, Command, discover_agents, discover_commands};
pub use config::{Hook, McpServer, discover_hooks, discover_mcp_servers};
pub use plugins::{Plugin, discover_plugins};
pub use sessions::{
    Kind, MAX_READ, Session, Status, discover_sessions, kind_of, opener_of, opening_turn,
    project_key, session_dirs, status, tail, tail_last, transcript_len, transcript_path,
};
pub use skills::{Skill, discover_skills, plugin_skills};

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

/// Where a piece of configuration comes from.
///
/// Scope decides precedence and, more importantly, trust: `Project` configuration arrives with the
/// repository and is authored by whoever wrote it, which is not necessarily the person running it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// `~/.claude` — the user's own configuration.
    User,
    /// `~/.claude.json` under this repository's own key — Claude Code's `local` scope. Private to
    /// this machine and this project, which is why it is the scope Keel writes to.
    Local,
    /// `<repo>/.claude` — travels with the repository.
    Project,
    /// Installed from a plugin marketplace.
    Plugin,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Local => "local",
            Scope::Project => "project",
            Scope::Plugin => "plugin",
        }
    }
}

/// Everything Keel found for one repository.
#[derive(Debug, Default, Serialize)]
pub struct Workspace {
    pub repo: Utf8PathBuf,
    pub sessions: Vec<Session>,
    pub skills: Vec<Skill>,
    pub plugins: Vec<Plugin>,
    pub agents: Vec<Agent>,
    pub commands: Vec<Command>,
    pub hooks: Vec<Hook>,
    pub mcp_servers: Vec<McpServer>,
}

impl Workspace {
    /// Discover everything for `repo`, using `claude_home` as the user-scope root.
    ///
    /// Every discovery step degrades to an empty list rather than failing: a machine with no
    /// plugins installed is not an error, and neither is one where the layout has moved on.
    pub fn discover(repo: impl AsRef<Utf8Path>, claude_home: &Utf8Path) -> Self {
        let repo = repo.as_ref();
        let plugins = discover_plugins(repo, claude_home);
        let mut skills = discover_skills(repo, claude_home);
        skills.extend(plugin_skills(&plugins));
        Self {
            repo: repo.to_owned(),
            sessions: discover_sessions(repo, claude_home),
            skills,
            plugins,
            agents: discover_agents(repo, claude_home),
            commands: discover_commands(repo, claude_home),
            hooks: discover_hooks(repo, claude_home),
            mcp_servers: discover_mcp_servers(repo, claude_home),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
            && self.skills.is_empty()
            && self.plugins.is_empty()
            && self.agents.is_empty()
            && self.commands.is_empty()
            && self.hooks.is_empty()
            && self.mcp_servers.is_empty()
    }

    /// Configuration that arrived with the repository and will execute.
    ///
    /// This is what `keel trust` quarantines, and what the UI marks in red.
    pub fn untrusted_count(&self) -> usize {
        self.hooks.iter().filter(|h| h.is_untrusted()).count()
            + self.mcp_servers.iter().filter(|s| s.is_untrusted()).count()
    }
}

/// The user's Claude Code home, honouring `CLAUDE_CONFIG_DIR`.
pub fn claude_home() -> Option<Utf8PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        return Some(Utf8PathBuf::from(dir));
    }
    Some(home()?.join(".claude"))
}

/// A path resolved the way the rest of the machine writes it. `canonicalize` on Windows returns
/// the verbatim form, `\\?\C:\work\app`, which git does not accept as `-C`, Claude Code does not
/// file transcripts under (so `project_key` found no sessions), and a person does not recognise.
/// The prefix is dropped when what follows is a plain drive path; a verbatim UNC path is kept,
/// because there the prefix is what makes it reachable. Everywhere Keel resolves a path it uses
/// this, so two resolved paths still compare equal.
pub trait Real {
    fn real(&self) -> std::io::Result<Utf8PathBuf>;
}

impl Real for Utf8Path {
    fn real(&self) -> std::io::Result<Utf8PathBuf> {
        self.canonicalize_utf8().map(plain)
    }
}

/// [`Real`] for a `std` path.
pub fn real_std(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let p = std::fs::canonicalize(path)?;
    Ok(match Utf8PathBuf::from_path_buf(p) {
        Ok(u) => plain(u).into_std_path_buf(),
        Err(p) => p,
    })
}

/// A path inside a repository the way git, the page and every other platform write it: `/`
/// between its parts. On Windows `strip_prefix` leaves `components\onboarding.tsx`, and the
/// imports, a skill's files and the project map all came back unmatched against `/` paths.
pub fn slashed(rel: &Utf8Path) -> String {
    rel.components()
        .map(|c| c.as_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// Drop `\\?\` before a drive letter; leave everything else as it is.
pub fn plain(path: Utf8PathBuf) -> Utf8PathBuf {
    if let Some(rest) = path.as_str().strip_prefix(r"\\?\")
        && rest.as_bytes().get(1) == Some(&b':')
        && rest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
    {
        return Utf8PathBuf::from(rest);
    }
    path
}

/// The person's home directory: `$HOME`, or the profile directory on Windows, where a process
/// started from the Start menu has no `HOME` at all.
///
/// `KEEL_HOME` first: where Keel keeps its own state and reads Claude Code's (`.keel`, `.claude`),
/// for a run that must not touch the person's — a test, a QA pass. It moves Keel's own lookups
/// and nothing else: the `HOME` the processes Keel starts inherit (git, `claude`, a dev server) is
/// left exactly as it was.
pub fn home() -> Option<Utf8PathBuf> {
    if let Some(keel) = std::env::var("KEEL_HOME").ok().filter(|h| !h.is_empty()) {
        return Some(Utf8PathBuf::from(keel));
    }
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(Utf8PathBuf::from)
        .or_else(|| std::env::home_dir().and_then(|h| Utf8PathBuf::from_path_buf(h).ok()))
}

/// Read the `name` and `description` fields out of a Markdown file's YAML frontmatter.
///
/// Deliberately a hand-rolled scan of the leading `---` block rather than a YAML dependency: these
/// files have two fields worth reading, and a malformed one should yield `None` rather than take
/// the listing down.
pub(crate) fn frontmatter(contents: &str) -> (Option<String>, Option<String>) {
    let mut lines = contents.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }

    let (mut name, mut description) = (None, None);
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("name:") {
            name = Some(scalar(rest));
        } else if let Some(rest) = trimmed.strip_prefix("description:") {
            description = Some(scalar(rest));
        }
    }
    (name, description)
}

/// A one-line YAML scalar as its text: a double-quoted one has its `\"` and `\\` undone, so a
/// description Keel wrote quoted reads back as it was typed.
fn scalar(raw: &str) -> String {
    let raw = raw.trim();
    let Some(inner) = raw
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .filter(|_| raw.len() >= 2)
    else {
        return raw.trim_matches('"').to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some(n @ ('"' | '\\'))) => {
                out.push(n);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// Truncate on a character boundary, appending an ellipsis when anything was removed.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keel quotes what it writes (`agents::yaml`); the list must show it as it was typed.
    #[test]
    fn a_quoted_description_reads_back_unescaped() {
        let (_, d) = frontmatter("---\nname: n\ndescription: \"say \\\"hi\\\": a \\\\ b\"\n---\n");
        assert_eq!(d.as_deref(), Some("say \"hi\": a \\ b"));
        let (_, plain) = frontmatter("---\ndescription: C:\\path\n---\n");
        assert_eq!(plain.as_deref(), Some("C:\\path"));
    }

    #[test]
    fn reads_frontmatter_fields() {
        let (name, description) =
            frontmatter("---\nname: video-gen\ndescription: Makes videos\n---\n\n# body");
        assert_eq!(name.as_deref(), Some("video-gen"));
        assert_eq!(description.as_deref(), Some("Makes videos"));
    }

    #[test]
    fn a_file_without_frontmatter_yields_nothing() {
        assert_eq!(frontmatter("# just a heading\n"), (None, None));
    }

    #[test]
    fn truncate_respects_character_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
    }
}

#[cfg(test)]
mod real_tests {
    use super::*;

    /// The verbatim drive form is the one git and Claude Code do not write; UNC keeps its prefix.
    #[test]
    fn a_verbatim_drive_path_is_written_plainly() {
        assert_eq!(
            plain(r"\\?\C:\work\app".into()),
            Utf8PathBuf::from(r"C:\work\app")
        );
        assert_eq!(
            plain(r"\\?\UNC\server\share".into()),
            Utf8PathBuf::from(r"\\?\UNC\server\share")
        );
        assert_eq!(
            plain("/Users/me/app".into()),
            Utf8PathBuf::from("/Users/me/app")
        );
    }
}
