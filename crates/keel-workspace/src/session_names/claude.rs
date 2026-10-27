//! Claude's explicit title record and sidecar, written without changing conversation messages.
use super::{NativeName, files};
use camino::Utf8Path;
use std::io::{Read, Seek, SeekFrom, Write};

pub(super) fn read(home: &Utf8Path, cwd: &Utf8Path, id: &str) -> Option<NativeName> {
    let s = crate::session_summary(cwd, home, id)?;
    Some(NativeName {
        fingerprint: format!(
            "{}:{:?}:{}",
            s.title_revision,
            s.title_source,
            s.title.as_deref().unwrap_or_default()
        ),
        explicit: s.title_source == Some("explicit"),
        title: s.title,
        last_active: s.last_active,
        messages: s.messages,
    })
}

pub(super) fn rename(home: &Utf8Path, cwd: &Utf8Path, id: &str, name: &str) -> Result<(), String> {
    let path = crate::transcript_path(cwd, home, id)
        .ok_or("The session transcript is not available yet")?;
    files::checked(home, &path)?;
    let metadata = path
        .parent()
        .ok_or("No session directory")?
        .join(id)
        .join("custom-title.json");
    files::checked(home, &metadata)?;
    let same = read(home, cwd, id).is_some_and(|n| n.explicit && n.title.as_deref() == Some(name));
    if !same {
        append(&path, id, name)?;
    }
    let body =
        serde_json::to_vec(&serde_json::json!({"customTitle":name})).map_err(|e| e.to_string())?;
    files::replace(home, &metadata, &body)
}

fn append(path: &Utf8Path, id: &str, name: &str) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 0 {
        file.seek(SeekFrom::End(-1)).map_err(|e| e.to_string())?;
        let mut end = [0];
        file.read_exact(&mut end).map_err(|e| e.to_string())?;
        if end != *b"\n" {
            return Err("The transcript is still being written; rename again to retry".into());
        }
    }
    let mut line = serde_json::to_vec(
        &serde_json::json!({"type":"custom-title","customTitle":name,"sessionId":id}),
    )
    .map_err(|e| e.to_string())?;
    line.push(b'\n');
    let written = file.write(&line).map_err(|e| e.to_string())?;
    if written != line.len() {
        return Err("The session name write was incomplete".into());
    }
    file.sync_data().map_err(|e| e.to_string())
}
