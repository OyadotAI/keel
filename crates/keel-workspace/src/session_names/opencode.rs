//! Read OpenCode's session title from its database; Keel never writes there, so no rename.
use super::{NativeName, files};
use camino::Utf8Path;

/// What OpenCode calls a session until its title model has named it.
const UNNAMED: &str = "New session - ";

/// The session row alone — title, time, a count — and never a message's `data`.
pub(super) fn read(db: &Utf8Path, cwd: &Utf8Path, id: &str) -> Result<Option<NativeName>, String> {
    if !crate::opencode::is_session_id(id) {
        return Err("That is not a session id".into());
    }
    if !db.exists() {
        return Ok(None);
    }
    let c = files::database(db)?;
    let row = c.query_row(
        "SELECT title, directory, time_updated, \
         (SELECT count(*) FROM message WHERE session_id = ?1) FROM session WHERE id = ?1",
        [id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        },
    );
    let (title, recorded, updated, messages) = match row {
        Ok(row) => row,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if !crate::moved::same_place(Utf8Path::new(&recorded), cwd) {
        return Err("The OpenCode session belongs to another checkout".into());
    }
    let explicit = !title.is_empty() && !title.starts_with(UNNAMED);
    Ok(Some(NativeName {
        fingerprint: format!("{explicit}:{title}"),
        title: (!title.is_empty()).then_some(title),
        explicit,
        last_active: Some(crate::iso(updated.div_euclid(1000), 0)),
        messages: messages as usize,
    }))
}
