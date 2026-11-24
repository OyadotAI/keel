//! Thread and comment connections have independent cursors; partial traversal never resolves old items.
use super::{
    Read,
    constants::*,
    transport::{clipped, text},
};
use crate::delivery::{RemoteComment, RemoteThread};
use serde_json::{Value, json};

const THREADS: &str = "query($owner:String!,$name:String!,$number:Int!,$cursor:String,$threads:Int!,$comments:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){headRefOid reviewThreads(first:$threads,after:$cursor){pageInfo{endCursor hasNextPage} nodes{id isResolved isOutdated comments(first:$comments){pageInfo{endCursor hasNextPage} nodes{id url body path line originalLine originalCommit{oid}}}}}}}}";
const COMMENTS: &str = "query($id:ID!,$cursor:String,$comments:Int!){node(id:$id){... on PullRequestReviewThread{comments(first:$comments,after:$cursor){pageInfo{endCursor hasNextPage} nodes{id url body path line originalLine originalCommit{oid}}}}}}";

pub(super) fn read(
    reader: &mut impl Read,
    repository: &str,
    number: u64,
    sha: &str,
) -> (Vec<RemoteThread>, Vec<String>) {
    let mut output = Vec::new();
    let result = collect(reader, repository, number, sha, &mut output);
    (output, result.err().into_iter().collect())
}

fn collect(
    reader: &mut impl Read,
    repository: &str,
    number: u64,
    sha: &str,
    output: &mut Vec<RemoteThread>,
) -> Result<(), String> {
    let (owner, name) = repository
        .split_once('/')
        .ok_or("Invalid pull request repository.")?;
    let mut cursor = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut count = 0;
    loop {
        let result = reader.graphql(THREADS, json!({"owner":owner,"name":name,"number":number,"cursor":cursor,"threads":THREAD_PAGE,"comments":COMMENT_PAGE}))?;
        let pr = &result["data"]["repository"]["pullRequest"];
        if pr["headRefOid"].as_str() != Some(sha) {
            return Err(
                "Pull request head changed during feedback refresh. Refresh again before sending."
                    .into(),
            );
        }
        let connection = &pr["reviewThreads"];
        let nodes = connection["nodes"]
            .as_array()
            .ok_or("GitHub omitted review threads.")?;
        for node in nodes {
            count += 1;
            if count > RECORD_LIMIT {
                return Err("Feedback refresh incomplete — review traversal limit reached.".into());
            }
            let id = text(node, "id")?;
            if output.iter().any(|t| t.id == id) {
                return Err("GitHub repeated a review thread.".into());
            }
            let comments = comments(reader, &id, &node["comments"], &mut count)?;
            output.push(RemoteThread {
                id,
                resolved: node["isResolved"]
                    .as_bool()
                    .ok_or("GitHub omitted thread resolution.")?,
                outdated: node["isOutdated"]
                    .as_bool()
                    .ok_or("GitHub omitted thread currency.")?,
                comments,
            });
        }
        cursor = next(connection, &mut seen)?;
        if cursor.is_none() {
            return Ok(());
        }
    }
}

fn comments(
    reader: &mut impl Read,
    id: &str,
    first: &Value,
    count: &mut usize,
) -> Result<Vec<RemoteComment>, String> {
    let mut connection = first.clone();
    let mut seen = std::collections::BTreeSet::new();
    let mut output = Vec::new();
    loop {
        for node in connection["nodes"]
            .as_array()
            .ok_or("GitHub omitted thread comments.")?
        {
            *count += 1;
            if *count > RECORD_LIMIT {
                return Err(
                    "Feedback refresh incomplete — comment traversal limit reached.".into(),
                );
            }
            let (body, truncated) = clipped(&text(node, "body")?);
            if truncated {
                return Err(
                    "Feedback refresh incomplete — a review comment exceeded its body limit."
                        .into(),
                );
            }
            let id = text(node, "id")?;
            if output.iter().any(|c: &RemoteComment| c.id == id) {
                return Err("GitHub repeated a review comment.".into());
            }
            output.push(RemoteComment {
                id,
                url: text(node, "url")?,
                body,
                path: text(node, "path")?,
                line: line(&node["line"])?,
                original_line: line(&node["originalLine"])?,
                original_commit: node["originalCommit"]["oid"].as_str().map(str::to_string),
            });
        }
        let Some(cursor) = next(&connection, &mut seen)? else {
            return Ok(output);
        };
        connection = reader.graphql(
            COMMENTS,
            json!({"id":id,"cursor":cursor,"comments":COMMENT_PAGE}),
        )?["data"]["node"]["comments"]
            .clone();
    }
}

fn line(value: &Value) -> Result<Option<u32>, String> {
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n != 0)
        .map(Some)
        .ok_or("GitHub returned an invalid review location.".into())
}
fn next(
    connection: &Value,
    seen: &mut std::collections::BTreeSet<String>,
) -> Result<Option<String>, String> {
    if !connection["pageInfo"]["hasNextPage"]
        .as_bool()
        .ok_or("GitHub omitted pagination completeness.")?
    {
        return Ok(None);
    }
    let cursor = text(&connection["pageInfo"], "endCursor")?;
    if cursor.is_empty() || !seen.insert(cursor.clone()) {
        return Err("GitHub repeated a pagination cursor.".into());
    }
    Ok(Some(cursor))
}
