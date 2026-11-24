//! Frame headings and actionable numbers share a byte budget; omissions remain explicit.
use super::constants::{
    LABEL_BYTES, READ_TEXT_BYTES as TEXT_BUDGET, ROW_RESERVE, SECTION_RESERVE, URL_BYTES,
};
use super::frames::{self, Frame};
use serde_json::{Value, json};
/// The default read is top-level; the opt-in read enumerates independently registered native frames.
pub async fn read(view: &tauri::Webview, all: bool) -> Result<(String, Value), String> {
    frames::begin_read(view);
    let mut inventory = frames::inventory(view).await?;
    inventory.sort_by_key(|f| !f.main);
    if !all {
        inventory.retain(|f| f.main);
    }
    if inventory.is_empty() {
        return Err(
            "No diagnostic frame has registered yet. Wait for loading, then read again.".into(),
        );
    }
    let budget = TEXT_BUDGET / inventory.len();
    let mut text = String::new();
    let mut rows = Vec::new();
    let mut count = 0;
    for frame in &inventory {
        let heading = format!(
            "## Frame {}\nURL: {}\n",
            frame.reference,
            prefix(&frame.url, URL_BYTES)
        );
        text.push_str(&heading);
        match frames::run(view, frame, "read", json!({})).await {
            Ok(data) => {
                text.push_str(&body(
                    view,
                    frame,
                    &data,
                    budget.saturating_sub(heading.len() + SECTION_RESERVE),
                )?);
                count += 1;
                rows.push(json!({"ref":frame.reference,"url":prefix(&frame.url,URL_BYTES),"readable":true}));
            }
            Err(why) => {
                text.push_str(&format!("Unread: {}\n", prefix(&why, SECTION_RESERVE)));
                rows.push(json!({"ref":frame.reference,"url":prefix(&frame.url,URL_BYTES),"readable":false}));
            }
        }
        text.push('\n');
    }
    text.push_str(&format!(
        "Frames: {count} read; {} unread.",
        inventory.len() - count
    ));
    Ok((
        text,
        json!({"frames":rows,"read":count,"unread":inventory.len()-count}),
    ))
}
fn body(
    view: &tauri::Webview,
    frame: &Frame,
    data: &Value,
    budget: usize,
) -> Result<String, String> {
    let mut lines = String::new();
    let elements = data["elements"].as_array().cloned().unwrap_or_default();
    let mut count = 0;
    for row in &elements {
        let line = format!(
            "{} {}{}",
            row["role"].as_str().unwrap_or("element"),
            prefix(&row["name"].to_string(), LABEL_BYTES),
            if row["covered"] == true {
                " (covered)"
            } else {
                ""
            }
        );
        if lines.len() + line.len() + ROW_RESERVE > budget {
            break;
        }
        let local = row["id"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .ok_or("Invalid isolated node reference.")?;
        lines.push_str(&format!(
            "[{}] {line}\n",
            frames::publish(view, frame, local)?
        ));
        count += 1;
    }
    let text = prefix(
        data["text"].as_str().unwrap_or(""),
        budget.saturating_sub(lines.len()),
    );
    Ok(format!(
        "{text}\nElements:\n{lines}{} elements omitted by the result limit.\n",
        elements.len() - count
    ))
}
/// Truncation ends at a UTF-8 boundary and labels shortened metadata.
pub fn prefix(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let mut end = limit.saturating_sub(24);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [shortened]", &text[..end])
}
