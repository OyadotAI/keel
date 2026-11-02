//! Local Git refs and worktree occupancy support branch discovery without a network fetch.
mod actions;
#[cfg(test)]
mod tests;
mod worktrees;
use super::{git, git_run};
pub use actions::git_branch_target;
use camino::Utf8Path;
use serde::Serialize;
use worktrees::Worktree;

/// Additional ref metadata is additive so older Git clients keep their existing response shape.
#[derive(Clone, Debug, Serialize)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub upstream: Option<String>,
    pub upstream_ref: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub subject: String,
    pub when: Option<i64>,
    pub worktree: Option<Worktree>,
}

/// A recorded remote ref includes its tracking locals, never an automatically fetched inventory.
#[derive(Debug, Serialize)]
pub struct RemoteBranch {
    pub name: String,
    pub remote: String,
    pub subject: String,
    pub when: Option<i64>,
    pub tracking: Vec<String>,
}

/// Preserve legacy fields while reporting detached HEAD and restrictions explicitly.
#[derive(Serialize)]
pub struct Branches {
    pub current: Option<String>,
    pub head: Option<String>,
    pub local: Vec<Branch>,
    pub remote: Vec<String>,
    pub remote_details: Vec<RemoteBranch>,
    pub remotes: Vec<String>,
    pub staged: u32,
    pub unstaged: u32,
    pub restriction: Option<String>,
}

const REF_FORMAT: &str = "--format=%(refname)%00%(refname:short)%00%(upstream:short)%00%(upstream:track)%00%(committerdate:unix)%00%(symref)%00%(upstream)%00%(subject)";

/// Fail a refresh visibly instead of confusing a Git error with an empty repository.
pub fn git_branches(root: &Utf8Path) -> Result<Branches, String> {
    let raw = git_run(
        root,
        &["for-each-ref", REF_FORMAT, "refs/heads", "refs/remotes"],
    )?;
    let current =
        git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).map(|s| s.trim().to_string());
    let occupied = worktrees::read(root)?;
    let remotes: Vec<String> = git_run(root, &["remote"])?
        .lines()
        .map(str::to_string)
        .collect();
    let rows: Vec<Vec<&str>> = raw
        .lines()
        .map(|line| line.splitn(8, '\0').collect::<Vec<_>>())
        .filter(|row| row.len() == 8)
        .collect();
    let local: Vec<Branch> = rows
        .iter()
        .filter(|row| row[0].starts_with("refs/heads/"))
        .map(|row| local_branch(row, current.as_deref(), &occupied))
        .collect();
    let remote_details: Vec<RemoteBranch> = rows
        .iter()
        .filter(|row| row[0].starts_with("refs/remotes/") && row[5].is_empty())
        .map(|row| {
            let name = row[0].trim_start_matches("refs/remotes/").to_string();
            let remote = remotes
                .iter()
                .filter(|r| name.starts_with(&format!("{r}/")))
                .max_by_key(|r| r.len())
                .cloned()
                .unwrap_or_else(|| name.split('/').next().unwrap_or("").into());
            RemoteBranch {
                tracking: local
                    .iter()
                    .filter(|b| {
                        b.upstream_ref.as_deref() == Some(format!("refs/remotes/{name}").as_str())
                    })
                    .map(|b| b.name.clone())
                    .collect(),
                name,
                remote,
                when: row[4].parse().ok(),
                subject: row[7].into(),
            }
        })
        .collect();
    let remote = remote_details
        .iter()
        .filter(|b| b.tracking.is_empty() && !b.name.ends_with("/HEAD"))
        .map(|b| b.name.clone())
        .collect();
    let status = git_run(root, &["status", "--porcelain"])?;
    Ok(Branches {
        current,
        head: git(root, &["rev-parse", "--short", "HEAD"]).map(|s| s.trim().to_string()),
        local,
        remote,
        remote_details,
        remotes,
        staged: status
            .lines()
            .filter(|l| l.len() > 1 && !l.starts_with([' ', '?']))
            .count() as u32,
        unstaged: status
            .lines()
            .filter(|l| l.len() > 1 && (l.as_bytes()[1] != b' ' || l.starts_with("??")))
            .count() as u32,
        restriction: None,
    })
}

fn local_branch(row: &[&str], current: Option<&str>, occupied: &[Worktree]) -> Branch {
    let name = row[0].trim_start_matches("refs/heads/").to_string();
    let count = |key: &str| {
        row[3]
            .split(|c: char| !c.is_alphanumeric())
            .collect::<Vec<_>>()
            .windows(2)
            .find(|w| w[0] == key)
            .and_then(|w| w[1].parse().ok())
            .unwrap_or(0)
    };
    Branch {
        current: current == Some(name.as_str()),
        ahead: count("ahead"),
        behind: count("behind"),
        upstream: (!row[2].is_empty()).then(|| row[2].into()),
        upstream_ref: (!row[6].is_empty()).then(|| row[6].into()),
        when: row[4].parse().ok(),
        subject: row[7].into(),
        worktree: occupied
            .iter()
            .find(|w| w.branch.as_deref() == Some(&name))
            .cloned(),
        name,
    }
}

/// The original caller contract still works while the new UI supplies an explicit ref kind.
pub fn git_branch_act(root: &Utf8Path, action: &str, name: &str) -> Result<String, String> {
    git_branch_target(root, action, name, None)
}
