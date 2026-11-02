//! NUL-delimited Git worktree records preserve paths with whitespace without shell parsing.
use camino::Utf8Path;
use serde::Serialize;

/// A registered worktree is the ownership proof; a branch-name pattern is never sufficient.
#[derive(Clone, Debug, Serialize)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    pub keel: Option<String>,
}

pub fn read(root: &Utf8Path) -> Result<Vec<Worktree>, String> {
    let text = crate::git::run(root, &["worktree", "list", "--porcelain", "-z"])?;
    let mut rows: Vec<Worktree> = text
        .split("\0\0")
        .filter_map(|record| {
            let fields: Vec<&str> = record.split('\0').collect();
            let path = fields.iter().find_map(|f| f.strip_prefix("worktree "))?;
            let branch = fields
                .iter()
                .find_map(|f| f.strip_prefix("branch refs/heads/"))
                .map(str::to_string);
            Some(Worktree {
                path: path.into(),
                branch,
                keel: None,
            })
        })
        .collect();
    if let Some(main) = rows.first().map(|w| camino::Utf8PathBuf::from(&w.path)) {
        for row in &mut rows {
            row.keel = Utf8Path::new(&row.path)
                .strip_prefix(main.join(crate::worktree::DIR))
                .ok()
                .filter(|p| p.components().count() == 1)
                .map(|p| p.as_str().to_string());
        }
    }
    Ok(rows)
}
