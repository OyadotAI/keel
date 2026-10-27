//! Antigravity title annotations are authoritative; summaries mirror them for its resume picker.
use super::{NativeName, files};
use camino::Utf8Path;
use rusqlite::OptionalExtension;

const METADATA_LIMIT: u64 = 64 * 1024;

pub(super) fn read(
    home: &Utf8Path,
    cwd: &Utf8Path,
    id: &str,
) -> Result<Option<NativeName>, String> {
    let db = home.join("conversation_summaries.db");
    if !db.exists() {
        return Ok(None);
    }
    let c = files::database(&db)?;
    let row = c.query_row("SELECT title, workspace_uris, last_modified_time, step_count FROM conversation_summaries WHERE conversation_id = ?1", [id], |r| {
        Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,usize>(3)?))
    }).optional().map_err(|e| e.to_string())?;
    let Some((summary, workspaces, at, messages)) = row else {
        return Ok(None);
    };
    let workspaces: Vec<String> = serde_json::from_str(&workspaces).map_err(|e| e.to_string())?;
    let uri = format!("file://{cwd}");
    if !workspaces.contains(&uri)
        && crate::antigravity::conversation(&home.join("conversations"), cwd, id).is_none()
    {
        return Err("The Antigravity session belongs to another checkout".into());
    }
    let text = annotation(home, id)?;
    let named = super::antigravity_proto::title(&text)?.map(|(_, name)| name);
    let explicit = named.as_ref().is_some_and(|t| !t.is_empty());
    let name = named.filter(|t| !t.is_empty()).unwrap_or(summary);
    Ok(Some(NativeName {
        fingerprint: format!("{explicit}:{name}"),
        title: (!name.is_empty()).then_some(name),
        explicit,
        last_active: Some(at),
        messages,
    }))
}

fn annotation(home: &Utf8Path, id: &str) -> Result<String, String> {
    use std::io::Read;
    let path = home.join("annotations").join(format!("{id}.pbtxt"));
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut text = String::new();
    file.take(METADATA_LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > METADATA_LIMIT {
        return Err("The title annotation is too large".into());
    }
    Ok(text)
}

pub(super) fn rename(home: &Utf8Path, cwd: &Utf8Path, id: &str, name: &str) -> Result<(), String> {
    read(home, cwd, id)?.ok_or("The Antigravity session is not available yet")?;
    let path = home.join("annotations").join(format!("{id}.pbtxt"));
    let old = annotation(home, id)?;
    let title = super::antigravity_proto::title(&old)?;
    let line = format!(
        "title:{}",
        serde_json::to_string(name).map_err(|e| e.to_string())?
    );
    let mut body = old;
    if let Some((range, _)) = title {
        body.replace_range(range, &line);
    } else {
        body.push_str(&format!("\n{line}\n"));
    }
    files::replace(home, &path, body.as_bytes())?;
    mirror(home, id, name)
}

fn mirror(home: &Utf8Path, id: &str, name: &str) -> Result<(), String> {
    let db = home.join("conversation_summaries.db");
    files::checked(home, &db)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        files::checked(home, &camino::Utf8PathBuf::from(format!("{db}{suffix}")))?;
    }
    let mut c =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|e| e.to_string())?;
    c.busy_timeout(std::time::Duration::from_millis(500))
        .map_err(|e| e.to_string())?;
    let tx = c.transaction().map_err(|e| e.to_string())?;
    let raw: Option<Vec<u8>> = tx
        .query_row(
            "SELECT raw_summary FROM conversation_summaries WHERE conversation_id = ?1",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let raw = raw
        .map(|b| super::antigravity_proto::summary(&b, name))
        .transpose()?;
    tx.execute(
        "UPDATE conversation_summaries SET title = ?1, raw_summary = ?2 WHERE conversation_id = ?3",
        rusqlite::params![name, raw, id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}
