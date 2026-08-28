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
//! transcript to render a list is not licence to display its contents. [`transcript`] is the
//! separate, explicit path for opening one session the user asked for by name.

mod agents;
mod config;
mod plugins;
mod sessions;
mod skills;

pub use agents::{Agent, Command, discover_agents, discover_commands};
pub use config::{Hook, McpServer, discover_hooks, discover_mcp_servers};
pub use plugins::{Plugin, discover_plugins};
pub use sessions::{
    Session, SessionCall, SessionWork, Turn, discover_sessions, project_key, session_work,
    transcript,
};
pub use skills::{Skill, discover_skills};

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
        Self {
            repo: repo.to_owned(),
            sessions: discover_sessions(repo, claude_home),
            skills: discover_skills(repo, claude_home),
            plugins: discover_plugins(claude_home),
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
    let home = std::env::var("HOME").ok()?;
    Some(Utf8PathBuf::from(home).join(".claude"))
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
            name = Some(rest.trim().trim_matches('"').to_string());
        } else if let Some(rest) = trimmed.strip_prefix("description:") {
            description = Some(rest.trim().trim_matches('"').to_string());
        }
    }
    (name, description)
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
