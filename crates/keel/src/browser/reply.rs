//! Browser replies preserve provenance and JSON values even when admission or execution fails.
use super::{Output, preference};
use camino::Utf8Path;
use serde_json::{Value, json};

/// A selection is configuration, never evidence that a browser executed this call.
pub(crate) fn selected(project: &Utf8Path, lane: Option<&str>) -> Option<&'static str> {
    match lane.and_then(|lane| preference::resolve(project, lane).ok().flatten()) {
        Some(preference::Backend::Embedded) => Some("embedded"),
        Some(preference::Backend::LocalOya) => Some("local_oya"),
        None => None,
    }
}

/// Only adapter-observed fields describe execution; a denied call keeps these fields unknown.
pub(super) fn identity(
    lane: Option<&str>,
    selected: Option<&str>,
    observed: Option<Value>,
) -> Value {
    let mut value = observed
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    for key in ["browser", "version", "engine", "profile", "dpr"] {
        if value.get(key).is_none() {
            value[key] = Value::Null;
        }
    }
    value["lane"] = json!(lane);
    let admitted = value["selected_backend"]
        .as_str()
        .or(selected)
        .map(str::to_string);
    value["selected"] = json!(admitted);
    if value.get("contacted").is_none() {
        value["contacted"] = Value::Null;
    }
    value
}

fn field<'a>(identity: &'a Value, key: &str) -> &'a str {
    identity[key].as_str().unwrap_or("unknown")
}

/// The compact caption is generated from the same record carried as structured MCP content.
fn caption(identity: &Value) -> String {
    let browser = if identity["contacted"] == true {
        field(identity, "browser")
    } else if identity["contacted"] == false {
        "not contacted"
    } else {
        "unknown"
    };
    let profile = identity["profile"]
        .as_object()
        .map(|_| {
            format!(
                "{} [{}]",
                field(&identity["profile"], "label"),
                field(&identity["profile"], "id")
            )
        })
        .unwrap_or("unknown".into());
    let dpr = identity["dpr"]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0)
        .map(|n| n.to_string())
        .unwrap_or("unknown".into());
    format!(
        "Lane: {} · Selected: {} · Browser: {browser} · Version: {} · Profile: {profile} · DPR: {dpr}",
        field(identity, "lane"),
        match identity["selected"].as_str() {
            Some("embedded") => "Inside Oya Factory",
            Some("local_oya") => "Oya Browser",
            _ => "unknown",
        },
        field(identity, "version")
    )
}

/// Every browser tool returns one envelope, including errors before the adapter was contacted.
pub(crate) fn tool_reply(
    result: Result<Output, String>,
    lane: Option<&str>,
    selected: Option<&str>,
) -> Value {
    let mut output = result.unwrap_or_else(|text| Output {
        text,
        ok: Some(false),
        ..Default::default()
    });
    let identity = identity(lane, selected, output.identity.take());
    let ok = output.ok.unwrap_or(true);
    let data = output.data.take();
    let outcome = output
        .outcome
        .take()
        .unwrap_or_else(|| if ok { "completed" } else { "failed" }.into());
    output.text = format!("{}\n\n{}", caption(&identity), output.text);
    json!({"content":output.content(), "isError":!ok,"structuredContent":{"identity":identity,"outcome":outcome,"data":data}})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denial_has_identity_without_claiming_browser_contact() {
        let reply = tool_reply(
            Ok(Output {
                text: "Denied".into(),
                ok: Some(false),
                identity: Some(json!({"contacted":false})),
                ..Default::default()
            }),
            Some("a"),
            Some("local_oya"),
        );
        assert_eq!(reply["isError"], true);
        assert_eq!(reply["structuredContent"]["identity"]["contacted"], false);
        assert_eq!(reply["structuredContent"]["identity"]["dpr"], Value::Null);
        assert!(
            reply["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("not contacted")
        );
    }
    #[test]
    fn an_unobserved_failure_does_not_claim_that_no_browser_was_contacted() {
        let reply = tool_reply(Err("Connection lost".into()), Some("a"), Some("local_oya"));
        assert_eq!(
            reply["structuredContent"]["identity"]["contacted"],
            Value::Null
        );
    }
    #[test]
    fn json_values_and_actual_failure_identity_survive_the_mcp_envelope() {
        let reply = tool_reply(
            Ok(Output {
                text: "Expression ran".into(),
                ok: Some(false),
                identity: Some(
                    json!({"contacted":true,"browser":"Oya Browser","version":"1.2.3","dpr":2}),
                ),
                data: Some(json!({"value":null})),
                ..Default::default()
            }),
            Some("a"),
            Some("local_oya"),
        );
        assert_eq!(reply["structuredContent"]["data"]["value"], Value::Null);
        assert_eq!(reply["structuredContent"]["identity"]["dpr"], 2);
        assert_eq!(reply["isError"], true);
    }
}
