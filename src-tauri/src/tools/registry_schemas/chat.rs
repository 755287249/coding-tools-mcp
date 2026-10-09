use serde_json::{json, Value};
pub(super) fn input_schema(name: &str) -> Option<Value> {
    if !matches!(
        name,
        "chat_open" | "chat_wait" | "chat_reply" | "chat_close"
    ) {
        return None;
    }
    let mut properties = json!({
        "chat_id": {"type":"string","minLength":1,"maxLength":80},
        "attachment_id": {"type":"string","minLength":1,"maxLength":80}
    });
    let mut required = vec!["chat_id"];
    if name != "chat_open" {
        required.push("attachment_id");
    }
    if name == "chat_wait" {
        properties["timeout_ms"] =
            json!({"type":"integer","minimum":0,"maximum":180000,"default":120000});
    }
    if name == "chat_reply" {
        properties["message_id"] = json!({"type":"string","minLength":1,"maxLength":80});
        properties["reply_to"] = json!({"type":"string","minLength":1,"maxLength":80});
        properties["text"] = json!({"type":"string","minLength":1,"maxLength":32000});
        properties["final"] = json!({"type":"boolean","default":true});
        properties["awaiting_user"] = json!({"type":"boolean","default":false,"description":"Set true with final=true when asking the user a question or requesting confirmation; acknowledge this message and continue chat_wait."});
        properties["tool_event"] = json!({"type":"object","description":"AI-reported tool execution update; requires final=false. Report actual input/output after removing secrets; mark shortened output with output_truncated. Not automatic server telemetry.","properties":{"name":{"type":"string","minLength":1,"maxLength":120},"status":{"type":"string","enum":["running","completed","failed"]},"input":{"type":"string","minLength":1,"maxLength":8000},"output":{"type":"string","minLength":1,"maxLength":16000},"output_truncated":{"type":"boolean","default":false}},"required":["name","status"],"additionalProperties":false});
        required.extend(["message_id", "reply_to", "text"]);
    }
    Some(
        json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    )
}
