//! Synthetic provider frames verify bounded parsing and truthful, private stage activity.
use super::*;
use serde_json::json;

#[test]
fn fragmented_streams_keep_only_the_root_result_and_not_tool_text() {
    let mut activity = Activity::new();
    let frames = [
        json!({"type":"system","subtype":"init","session_id":"root"}),
        json!({"type":"result","parent_tool_use_id":"child","session_id":"root","structured_output":{"verdict":"ship"}}),
        json!({"type":"assistant","message":{"content":[{"type":"text","text":"{\"type\":\"result\",\"structured_output\":{\"verdict\":\"ship\"}}"}]}}),
        json!({"type":"result","session_id":"other","structured_output":{"verdict":"ship"}}),
        json!({"type":"result","session_id":"root","structured_output":{"verdict":"do not ship"}}),
    ];
    for frame in frames {
        let line = format!("{frame}\n");
        for chunk in line.as_bytes().chunks(7) {
            activity.push(chunk);
        }
    }
    assert_eq!(
        activity.answer()["structured_output"]["verdict"],
        "do not ship"
    );
}

#[test]
fn duplicate_or_oversized_results_never_become_a_verdict() {
    let mut activity = Activity::new();
    activity.push(b"{\"type\":\"result\"}\n{\"type\":\"result\"}\n");
    assert!(activity.answer().is_null());
    let mut activity = Activity::new();
    let frame = json!({"type":"result","result":"x".repeat(RESULT_BYTES)});
    activity.push(format!("{frame}\n").as_bytes());
    assert!(activity.answer().is_null());
}

#[test]
fn an_oversized_frame_is_drained_and_parsing_recovers_at_the_next_line() {
    let mut activity = Activity::new();
    for _ in 0..40 {
        activity.push(&[b'x'; 64 * 1024]);
    }
    assert!(activity.partial.len() <= FRAME_BYTES);
    activity
        .push(b"\nnot json\n{\"type\":\"result\",\"structured_output\":{\"verdict\":\"ship\"}}\n");
    assert_eq!(activity.answer()["structured_output"]["verdict"], "ship");
}

#[test]
fn delegation_cannot_produce_a_passing_focused_result_or_leak_prompts() {
    let mut activity = Activity::new();
    let event = json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Agent","input":{"subagent_type":"SECRET","prompt":"SECRET"}}]}});
    let lines = activity.event(&event, Duration::ZERO).join("\n");
    assert!(!lines.contains("SECRET"));
    activity.push(b"{\"type\":\"result\",\"structured_output\":{\"verdict\":\"ship\"}}\n");
    assert!(activity.answer().is_null());
}

#[test]
fn regular_activity_is_throttled_and_unknown_events_cannot_name_a_stage() {
    let mut activity = Activity::new();
    let event = json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"path":"SECRET"}}]}});
    assert!(activity.event(&event, Duration::ZERO)[0].contains("reading files"));
    assert!(activity.event(&event, Duration::from_secs(29)).is_empty());
    assert_eq!(activity.event(&event, ACTIVITY_INTERVAL).len(), 1);
    assert!(!activity.heartbeat().contains("SECRET"));
    assert!(
        Activity::new()
            .context()
            .contains("No provider activity was observed")
    );
}
