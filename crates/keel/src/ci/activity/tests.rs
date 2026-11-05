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
fn stage_output_contains_only_known_names_and_observed_durations() {
    let mut activity = Activity::new();
    let call = |name, background| json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"a","name":"Agent","input":{"subagent_type":name,"prompt":"SECRET","run_in_background":background}}]}});
    assert!(
        !activity
            .event(&call("SECRET", false), Duration::ZERO)
            .join("\n")
            .contains("SECRET")
    );
    let start = activity
        .event(&call("reviewer", true), Duration::from_secs(12))
        .join("\n");
    assert!(start.contains("[0m 12s] stage reviewer started"));
    let result = json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a","content":"SECRET"}]}});
    assert!(
        !activity
            .event(&result, Duration::from_secs(20))
            .join("\n")
            .contains("finished")
    );
    let end = json!({"type":"system","subtype":"task_notification","tool_use_id":"a","status":"completed","summary":"SECRET"});
    let lines = activity.event(&end, Duration::from_secs(48)).join("\n");
    assert!(lines.contains("stage reviewer finished; took 0m 36s"));
    assert!(!lines.contains("SECRET"));
    assert!(activity.context().contains("Last observed stage: reviewer"));
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
            .contains("No stage activity was observed")
    );
}
