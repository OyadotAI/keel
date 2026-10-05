//! Creating a subagent or a slash command.
//!
//! Both are a Markdown file with YAML frontmatter — `.claude/agents/` and `.claude/commands/` — so
//! one writer serves both. The form exists because the frontmatter has required keys with meaning attached —
//! `description` is what the main agent reads to decide whether to delegate at all — and a file
//! created from a blank template gets those wrong quietly.
//!
//! There is deliberately no equivalent for hooks. A hook is a shell command Claude Code runs on
//! your behalf when a tool fires; a repository that ships one achieves code execution the moment
//! anybody opens it, which is why `keel-harness::trust` quarantines them and why the scanner rates
//! a repo-supplied `.claude/settings.json` Critical. Keel is not going to grow a button that
//! writes one.

use axum::{Json, extract::State, http::StatusCode};
use camino::{Utf8Path, Utf8PathBuf};
use keel_workspace::Real;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::serve::AppState;

#[derive(Deserialize)]
pub struct NewAgent {
    pub name: String,
    /// When the main agent should delegate to this one. Read by the model, not by a person.
    pub description: String,
    /// Comma-separated tool names. Empty means "inherit everything the main agent has".
    #[serde(default)]
    pub tools: String,
    /// The agent's own system prompt.
    #[serde(default)]
    pub prompt: String,
    /// `project` writes `.claude/agents`; `user` writes `~/.claude/agents`.
    #[serde(default)]
    pub scope: String,
    /// The person's "commit after every turn" setting; a project agent is then committed on
    /// its own rather than swept into the next turn's checkpoint under that turn's prompt.
    #[serde(default)]
    pub commit: bool,
}

/// What is being written. The two differ in folder, in frontmatter keys and in what a blank body
/// should say, and in nothing else.
#[derive(Clone, Copy)]
enum Kind {
    Agent,
    Command,
}

impl Kind {
    fn folder(self) -> &'static str {
        match self {
            Kind::Agent => ".claude/agents",
            Kind::Command => ".claude/commands",
        }
    }
}

#[derive(Serialize, Debug)]
pub struct Created {
    /// Repository-relative when it is in the project, absolute when it is in the user's home.
    pub path: String,
    /// Why it was not committed, when a commit was asked for and failed.
    pub note: Option<String>,
}

fn bad(m: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, m.to_string())
}

/// A name that is safe as a filename and valid as the frontmatter's `name`.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Quote a value for YAML.
///
/// A description is a sentence, and sentences contain colons. Unquoted, `Use this: for reviews`
/// parses as a nested mapping and the file loads with no description at all — which reads to the
/// main agent as an agent it has no reason to ever call.
///
/// One line, too: the field is a text editor, and a newline followed by `---` ends the
/// frontmatter inside the quotes, while one followed by `name: x` renames the file's owner to
/// every line-by-line reader. A description is one sentence to the model either way.
pub(crate) fn yaml(value: &str) -> String {
    // Control characters are not allowed in a YAML scalar at all, quoted or not: one `\x07` in a
    // description and the file loaded with no frontmatter.
    let value: String = value
        .chars()
        .filter(|c| !c.is_control() || c.is_whitespace())
        .collect();
    let line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("\"{}\"", line.replace('\\', "\\\\").replace('"', "\\\""))
}

pub async fn create(
    state: State<Arc<AppState>>,
    req: Json<NewAgent>,
) -> Result<Json<Created>, (StatusCode, String)> {
    // no-blocking: `make` does the write inside `blocking(…)`.
    make(state, req, Kind::Agent).await
}

/// A slash command: `/name` in Claude Code sends the file's body as the prompt.
pub async fn create_command(
    state: State<Arc<AppState>>,
    req: Json<NewAgent>,
) -> Result<Json<Created>, (StatusCode, String)> {
    // no-blocking: `make` does the write inside `blocking(…)`.
    make(state, req, Kind::Command).await
}

async fn make(
    State(state): State<Arc<AppState>>,
    Json(req): Json<NewAgent>,
    kind: Kind,
) -> Result<Json<Created>, (StatusCode, String)> {
    let repo = state.repo();
    crate::serve::blocking(
        move || {
            // A project file is a write into the tree a turn may be writing; a user one is not.
            let _held = if req.scope == "user" {
                None
            } else {
                Some(crate::writes::hold(&state, &repo, "agent")?)
            };
            let commit = req.commit && req.scope != "user";
            let rel = format!("{}/{}.md", kind.folder(), req.name);
            let mut made = write(&repo, req, kind)?;
            if commit {
                let message = match kind {
                    Kind::Agent => "Add a subagent",
                    Kind::Command => "Add a slash command",
                };
                made.note = crate::writes::commit_only(&repo, &[rel], message).note;
            }
            Ok(made)
        },
        Err(bad("the write was cancelled")),
    )
    .await
    .map(Json)
}

/// What `create` does, off the executor: it creates directories, checks for a file and writes.
fn write(repo: &Utf8Path, req: NewAgent, kind: Kind) -> Result<Created, (StatusCode, String)> {
    if !valid_name(&req.name) {
        return Err(bad(
            "Use lowercase letters, digits and hyphens, at most 64 characters — the name becomes \
             a filename.",
        ));
    }
    let description = req.description.trim();
    if description.is_empty() {
        return Err(bad(match kind {
            Kind::Agent => {
                "A description is required: it is the only thing the main agent reads when \
                 deciding whether to delegate."
            }
            Kind::Command => {
                "A description is required: it is what the / menu shows beside the name."
            }
        }));
    }

    let dir: Utf8PathBuf = if req.scope == "user" {
        let home = keel_workspace::home()
            .map(String::from)
            .ok_or(std::env::VarError::NotPresent)
            .map_err(|_| bad("no home directory"))?;
        Utf8PathBuf::from(home).join(kind.folder())
    } else {
        repo.join(kind.folder())
    };
    let path = dir.join(format!("{}.md", req.name));
    if req.scope != "user" {
        crate::writes::no_link_under(repo, &format!("{}/{}.md", kind.folder(), req.name))
            .map_err(|e| (StatusCode::CONFLICT, e))?;
    }

    // A command is named by its filename; only an agent carries `name` in its frontmatter.
    let mut front = match kind {
        Kind::Agent => format!(
            "---\nname: {}\ndescription: {}\n",
            req.name,
            yaml(description)
        ),
        Kind::Command => format!("---\ndescription: {}\n", yaml(description)),
    };
    // A tool list is one line of frontmatter; a newline in it writes whatever keys follow.
    if req.tools.chars().any(char::is_control) {
        return Err(bad("Tools are a comma-separated list on one line."));
    }
    let tools: Vec<&str> = req
        .tools
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    if !tools.is_empty() {
        let key = match kind {
            Kind::Agent => "tools",
            Kind::Command => "allowed-tools",
        };
        // Quoted: `Bash(git commit: *)` and a `#` are YAML syntax unquoted, and the file then
        // loaded with no frontmatter at all.
        front.push_str(&format!("{key}: {}\n", yaml(&tools.join(", "))));
    }
    front.push_str("---\n\n");

    let body = if req.prompt.trim().is_empty() && matches!(kind, Kind::Command) {
        "Write the prompt this command sends. $ARGUMENTS is whatever is typed after the \
         command's name.\n"
            .to_string()
    } else if req.prompt.trim().is_empty() {
        // A placeholder that says what belongs here rather than an empty file. An agent whose
        // prompt is blank inherits nothing useful and behaves like a worse copy of the main one.
        "Describe what this agent does, what it should read before acting, and what it should \
         return. Be specific about the shape of its answer — a subagent's reply is consumed by \
         another model, not by a person.\n"
            .to_string()
    } else {
        format!("{}\n", req.prompt.trim())
    };

    // Never over a file, never through a link, and two creates of one name cannot both win.
    if !crate::writes::create_new(&path, (front + &body).as_bytes()).map_err(bad)? {
        return Err((
            StatusCode::CONFLICT,
            format!("{}.md already exists — pick another name.", req.name),
        ));
    }

    let shown = match repo.real() {
        Ok(root) => path
            .strip_prefix(&root)
            .map(|p| p.to_string())
            .unwrap_or_else(|_| path.to_string()),
        Err(_) => path.to_string(),
    };
    Ok(Created {
        path: shown,
        note: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(name: &str, tools: &str) -> NewAgent {
        NewAgent {
            name: name.into(),
            description: "d".into(),
            tools: tools.into(),
            prompt: String::new(),
            scope: String::new(),
            commit: false,
        }
    }

    /// A newline in the tool list wrote `name:` and `description:` of the caller's choosing.
    #[test]
    fn a_tool_list_cannot_add_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8Path::from_path(dir.path()).unwrap();
        let err = write(repo, ask("tooly", "Read\nname: hijack"), Kind::Agent).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(!repo.join(".claude/agents/tooly.md").exists());
        write(repo, ask("tooly", "Read, Bash(git commit:*)"), Kind::Agent).unwrap();
    }

    /// Check-then-write let two creates of one name both succeed, the last silently winning.
    #[test]
    fn a_second_create_of_one_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8Path::from_path(dir.path()).unwrap();
        write(repo, ask("a", ""), Kind::Agent).unwrap();
        let err = write(repo, ask("a", "Read"), Kind::Agent).unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
        let body = std::fs::read_to_string(repo.join(".claude/agents/a.md")).unwrap();
        assert!(!body.contains("tools:"), "{body}");
    }

    /// A command is named by its file and allows tools under Claude Code's own key for it.
    #[test]
    fn a_command_is_written_where_the_slash_menu_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8Path::from_path(dir.path()).unwrap();
        write(repo, ask("ship", "Bash(git push:*)"), Kind::Command).unwrap();
        let body = std::fs::read_to_string(repo.join(".claude/commands/ship.md")).unwrap();
        assert!(
            body.starts_with("---\ndescription: \"d\"\nallowed-tools: \"Bash(git push:*)\"\n---"),
            "{body}"
        );
        assert!(body.contains("$ARGUMENTS"), "{body}");
    }

    /// Both of these wrote frontmatter that no YAML parser loads.
    #[test]
    fn frontmatter_stays_yaml_whatever_is_typed() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Utf8Path::from_path(dir.path()).unwrap();
        write(repo, ask("odd", "Bash(x: y), #Read"), Kind::Agent).unwrap();
        let body = std::fs::read_to_string(repo.join(".claude/agents/odd.md")).unwrap();
        assert!(body.contains("tools: \"Bash(x: y), #Read\"\n"), "{body}");
        let mut bell = ask("bell", "");
        bell.description = "ring\u{7} the \u{1b}[31mbell".into();
        write(repo, bell, Kind::Command).unwrap();
        let body = std::fs::read_to_string(repo.join(".claude/commands/bell.md")).unwrap();
        assert!(
            !body.chars().any(|c| c.is_control() && c != '\n'),
            "{body:?}"
        );
    }

    #[test]
    fn a_name_has_to_work_as_a_filename() {
        assert!(valid_name("code-reviewer"));
        assert!(!valid_name("Code Reviewer"));
        assert!(!valid_name("../escape"));
        assert!(!valid_name("-leading"));
        assert!(!valid_name(""));
    }

    /// A description is a sentence and sentences contain colons. Unquoted, YAML reads
    /// `Use this: for reviews` as a nested mapping and the description is lost — and an agent with
    /// no description is one the main agent has no reason to ever call.
    #[test]
    fn a_description_survives_its_own_punctuation() {
        assert_eq!(yaml("plain"), "\"plain\"");
        assert_eq!(
            yaml("Use this: after any change"),
            "\"Use this: after any change\""
        );
        assert_eq!(yaml("says \"hi\""), "\"says \\\"hi\\\"\"");
        // A newline is how the text editor ends a line, and how YAML ends the frontmatter.
        assert_eq!(yaml("notes\n---\nname: x"), "\"notes --- name: x\"");
    }
}
