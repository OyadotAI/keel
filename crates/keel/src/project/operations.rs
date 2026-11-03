//! Creation receipts retain outcomes so retrying a lost response cannot scaffold another project.
use super::{Created, NewProject};
use axum::http::StatusCode;
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
const RECEIPT_BYTES: u64 = 16 * 1024;
#[derive(Serialize, Deserialize)]
struct Receipt {
    hash: String,
    outcome: Option<Created>,
    error: Option<String>,
}
/// A receipt reserves an operation before filesystem writes and replays only a confirmed outcome.
pub(super) fn run(
    root: &Utf8Path,
    id: &str,
    request: &NewProject,
    action: impl FnOnce() -> Result<Created, (StatusCode, String)>,
) -> Result<Created, (StatusCode, String)> {
    let bad = |e: String| (StatusCode::CONFLICT, e);
    if id.is_empty()
        || id.len() > 128
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(bad("Invalid project operation identity".into()));
    }
    let rel = format!(".keel/project-operations/{id}.json");
    crate::writes::no_link_under(root, &rel).map_err(bad)?;
    let path = root.join(&rel);
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(bad("Project operation receipt is a symbolic link.".into()));
    }
    let hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(request).map_err(|e| bad(e.to_string()))?)
    );
    crate::writes::locked(path.as_std_path(),|| {
        use std::io::Read;
        if let Ok(file)=std::fs::File::open(&path) {
            let mut body=Vec::new();file.take(RECEIPT_BYTES+1).read_to_end(&mut body).map_err(|e|bad(e.to_string()))?;
            if body.len() as u64>RECEIPT_BYTES{return Err(bad("Project receipt exceeds its limit.".into()));}
            let record:Receipt=serde_json::from_slice(&body).map_err(|_|bad("Could not read the project creation receipt. Existing files were preserved.".into()))?;
            if record.hash!=hash{return Err(bad("This project operation was already used with different options.".into()));}
            return match record.outcome {Some(created)=>Ok(created),None=>Err(bad(record.error.unwrap_or_else(||"Creation was interrupted. Inspect the requested location before creating another project.".into())))};
        }
        let mut receipt=Receipt{hash,outcome:None,error:None};
        persist(&path,&receipt).map_err(bad)?;
        let result=action();
        match &result {Ok(created)=>receipt.outcome=Some(created.clone()),Err((_,error))=>receipt.error=Some(error.clone())};
        persist(&path,&receipt).map_err(bad)?;
        result
    }).map_err(bad)?
}
fn persist(path: &Utf8Path, receipt: &Receipt) -> Result<(), String> {
    crate::writes::replace(
        path.as_std_path(),
        &serde_json::to_vec(receipt).map_err(|e| e.to_string())?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lost_creation_responses_replay_the_same_result_and_reject_changed_options() {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(temp.path()).unwrap();
        let mut request = NewProject {
            parent: root.to_string(),
            name: String::new(),
            operation: Some("operation".into()),
            template: None,
            notes: None,
            patterns: None,
            pack: None,
        };
        let first = run(root, "operation", &request, || {
            Ok(Created {
                name: "project-one".into(),
                path: root.join("project-one").to_string(),
            })
        })
        .unwrap();
        let second = run(root, "operation", &request, || {
            panic!("repeated creation must not scaffold")
        })
        .unwrap();
        assert_eq!(first.path, second.path);
        request.name = "different".into();
        assert!(
            run(root, "operation", &request, || panic!(
                "different options reused receipt"
            ))
            .is_err()
        );
    }
    #[test]
    fn interrupted_creation_preserves_its_failure_instead_of_retrying_over_partial_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = Utf8Path::from_path(temp.path()).unwrap();
        let request = NewProject {
            parent: root.to_string(),
            name: "demo".into(),
            operation: Some("operation".into()),
            template: None,
            notes: None,
            patterns: None,
            pack: None,
        };
        assert!(
            run(root, "operation", &request, || Err((
                StatusCode::BAD_REQUEST,
                "Partial project at demo; Git initialization failed".into()
            )))
            .is_err()
        );
        let error = run(root, "operation", &request, || {
            panic!("must preserve partial result")
        })
        .unwrap_err();
        assert!(error.1.contains("Partial project at demo"));
    }
}
