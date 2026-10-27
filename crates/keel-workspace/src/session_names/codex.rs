//! Read Codex's native name/preview precedence; its App Server exclusively owns writes.
use super::{NativeName, files};
use crate::Real;
use camino::Utf8Path;

pub(super) fn read(
    home: &Utf8Path,
    cwd: &Utf8Path,
    id: &str,
) -> Result<Option<NativeName>, String> {
    let mut databases: Vec<_> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| camino::Utf8PathBuf::from_path_buf(e.path()).ok())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.starts_with("state_") && n.ends_with(".sqlite"))
        })
        .collect();
    databases.sort();
    for path in databases.into_iter().rev() {
        let c = files::database(&path)?;
        let row = c.query_row(
            "SELECT name, title, cwd, updated_at FROM threads WHERE id = ?1 AND archived = 0",
            [id],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        );
        let (name, title, recorded, updated) = match row {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
            Err(e) => return Err(e.to_string()),
        };
        let recorded = Utf8Path::new(&recorded);
        if recorded.real().unwrap_or_else(|_| recorded.to_owned())
            != cwd.real().unwrap_or_else(|_| cwd.to_owned())
        {
            return Err("The Codex session belongs to another checkout".into());
        }
        let explicit = name.as_ref().is_some_and(|n| !n.is_empty());
        let title = name.filter(|n| !n.is_empty()).unwrap_or(title);
        return Ok(Some(NativeName {
            fingerprint: format!("{explicit}:{title}"),
            title: (!title.is_empty()).then_some(title),
            explicit,
            last_active: Some(crate::iso(updated, 0)),
            messages: 0,
        }));
    }
    Ok(None)
}
