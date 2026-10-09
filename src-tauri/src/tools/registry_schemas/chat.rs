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
        required.extend(["message_id", "reply_to", "text"]);
    }
    Some(
        json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    )
}
