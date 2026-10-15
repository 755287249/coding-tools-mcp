//! User control over work in progress: recall a message, pause the AI, or jump a
//! queued correction ahead. The AI is told on its next tool call; the attachment
//! and chat_wait loop stay alive, so this is "wait a moment", not a disconnect.
use super::{err, id, now, pending, Result};
use serde_json::{json, Value};

pub(super) const REASON_PAUSE: &str = "pause";
pub(super) const REASON_CORRECTION: &str = "correction";

/// A recalled or interrupted user message no longer needs an answer.
pub(super) fn stopped(m: &Value) -> bool {
    m["recalled_at"].as_u64().is_some() || m["interrupted_at"].as_u64().is_some()
}

fn ensure_work_chat(s: &Value) -> Result<()> {
    if s["closed"] == true {
        return Err(err("Conversation is closed"));
    }
    if super::group::grouped(s) {
        return Err(err("Recall and pause are only available in work chats"));
    }
    Ok(())
}

fn answered(s: &Value, message_id: &Value) -> bool {
    s["messages"].as_array().is_some_and(|messages| {
        messages
            .iter()
            .any(|r| r["role"] == "assistant" && &r["reply_to"] == message_id && r["final"] == true)
    })
}

/// UI actions: `recall`, `interrupt`, `jump_queue`. The caller saves the session.
/// Returns the recalled message (text and attachments) so the UI restores exactly
/// what was withdrawn, including a merged queue publication.
pub(super) fn ui(s: &mut Value, action: &str, args: &Value) -> Result<Value> {
    ensure_work_chat(s)?;
    let time = now();
    let mut recalled = Value::Null;
    match action {
        "recall" => {
            let message_id = json!(id(&args["message_id"])?);
            if let Some(index) = s["queue"]
                .as_array()
                .and_then(|q| q.iter().position(|m| m["id"] == message_id))
            {
                // Not yet published: drop it and keep the receipt so a delayed send retry
                // cannot bring it back.
                let message = s["queue"][index].clone();
                if message.get("discussion").is_some() {
                    return Err(err("Collaboration deliveries cannot be recalled"));
                }
                if s["queue_receipts"].is_null() {
                    s["queue_receipts"] = json!({});
                }
                s["queue_receipts"][message_id.as_str().unwrap()] =
                    json!(super::queued_fingerprint(
                        message["text"].as_str().unwrap_or(""),
                        message["attachments"]
                            .as_array()
                            .map(Vec::as_slice)
                            .unwrap_or(&[])
                    ));
                recalled = json!({"text":message["text"],"attachments":message["attachments"]});
                s["queue"].as_array_mut().unwrap().remove(index);
            } else {
                let already = answered(s, &message_id);
                let message = s["messages"]
                    .as_array_mut()
                    .and_then(|m| m.iter_mut().find(|m| m["id"] == message_id))
                    .ok_or_else(|| err("Message not found"))?;
                if message["role"] != "user" || message["kind"] == "connection_request" {
                    return Err(err("Only your own messages can be recalled"));
                }
                if already {
                    return Err(err("The AI already answered this message"));
                }
                if message["recalled_at"].is_null() {
                    message["recalled_at"] = json!(time);
                }
                recalled = json!({"text":message["text"],"attachments":message["attachments"]});
                let text = message["text"].as_str().unwrap_or("").to_owned();
                reset_derived_title(s, &text);
            }
        }
        "interrupt" => {
            let current = pending(s).ok_or_else(|| err("No task is in progress"))?;
            if current["kind"] == "connection_request" {
                return Err(err("Wait until the AI has connected"));
            }
            // Act only on the task the user saw, and only once the AI has it.
            if args.get("message_id").is_some_and(|v| !v.is_null()) && current["id"] != args["message_id"] {
                return Err(err("The task changed; refresh and try again"));
            }
            if current["received_at"].as_u64().is_none() {
                return Err(err("The AI has not picked up this message yet; recall it instead"));
            }
            mark_interrupted(s, &current["id"], REASON_PAUSE, time);
            // "Wait a moment": hold queued messages until the user sends or jumps one.
            s["queue_paused"] = json!(true);
        }
        "jump_queue" => {
            let message_id = json!(id(&args["message_id"])?);
            let index = s["queue"]
                .as_array()
                .and_then(|q| q.iter().position(|m| m["id"] == message_id))
                .ok_or_else(|| err("Message is no longer queued"))?;
            if s["queue"][index].get("discussion").is_some() {
                return Err(err("Collaboration deliveries cannot jump the queue"));
            }
            let mut message = s["queue"].as_array_mut().unwrap().remove(index);
            message["created_at"] = json!(time);
            s.as_object_mut().unwrap().remove("queue_paused");
            match pending(s) {
                Some(current) if current["kind"] == "connection_request" => {
                    return Err(err("Wait until the AI has connected"));
                }
                // The AI is working on it: stop that task, the correction goes next.
                Some(current) if current["received_at"].as_u64().is_some() => {
                    mark_interrupted(s, &current["id"], REASON_CORRECTION, time);
                    s["messages"].as_array_mut().unwrap().push(message);
                }
                // Not picked up yet: simply deliver the correction first.
                Some(current) => {
                    let messages = s["messages"].as_array_mut().unwrap();
                    let at = messages.iter().position(|m| m["id"] == current["id"]).unwrap_or(messages.len());
                    messages.insert(at, message);
                }
                None => s["messages"].as_array_mut().unwrap().push(message),
            }
        }
        _ => return Err(err("Unknown chat action")),
    }
    s["updated_at"] = json!(time);
    Ok(recalled)
}

/// Pause holds the queue; a new send or a jump releases it.
pub(super) fn queue_paused(s: &Value) -> bool {
    s["queue_paused"] == true
}
pub(super) fn release_queue(s: &mut Value) {
    if let Some(map) = s.as_object_mut() {
        map.remove("queue_paused");
    }
}

/// A title taken from the recalled text would still show it; restore the default.
fn reset_derived_title(s: &mut Value, text: &str) {
    let derived: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(36)
        .collect();
    if s["title_custom"] != true
        && s["title_agent_name"].is_null()
        && s["title"] == derived.as_str()
    {
        s["title"] = json!("新对话");
    }
}

fn mark_interrupted(s: &mut Value, message_id: &Value, reason: &str, time: u64) {
    if let Some(m) = s["messages"]
        .as_array_mut()
        .and_then(|m| m.iter_mut().find(|m| &m["id"] == message_id))
    {
        if m["interrupted_at"].is_null() {
            m["interrupted_at"] = json!(time);
            m["interrupt_reason"] = json!(reason);
        }
    }
}

/// The stopped message the AI may still be working on: it received it, the user
/// stopped it, and the AI has not called chat_wait/chat_reply since.
pub(crate) fn stop_notice(s: &Value) -> Option<Value> {
    if s["closed"] == true || super::group::grouped(s) {
        return None;
    }
    let m = s["messages"].as_array()?.iter().rev().find(|m| {
        m["role"] == "user"
            && m["received_at"].as_u64().is_some()
            && stopped(m)
            && m["stop_acknowledged_at"].is_null()
    })?;
    let recalled = m["recalled_at"].as_u64().is_some();
    let correction = m["interrupt_reason"] == REASON_CORRECTION;
    let message = if recalled {
        "用户撤回了你正在处理的消息。立即停止这项工作，不要回复它，也不要继续相关工具调用；马上调用 chat_wait 等待用户的新消息，会话保持连接。The user recalled the message you were working on: stop now and call chat_wait."
    } else if correction {
        "用户发来了纠正，替换了你正在处理的任务。立即停止当前工作，可用一条简短 chat_reply 说明已停在哪里，然后马上调用 chat_wait 接收纠正内容。The user sent a correction: stop and call chat_wait to receive it."
    } else {
        "用户请你暂停（稍等）。立即停止当前工作，可用一条简短 chat_reply 说明进度和停在哪里，然后马上调用 chat_wait 等待用户的下一条消息；不要结束会话。The user paused you: stop, optionally post a short status reply, then call chat_wait."
    };
    Some(json!({
        "ok": false,
        "error": {
            "code": if recalled { "USER_RECALLED" } else if correction { "USER_CORRECTION" } else { "USER_PAUSED" },
            "category": "user_control",
            "retryable": false,
            "message_id": m["id"],
            "message": message
        }
    }))
}

/// Gateway check before a project tool call from the chat's AI.
pub(crate) fn stop_notice_at(root: &std::path::Path, chat_id: &str) -> Option<Value> {
    super::load(root, chat_id).ok().and_then(|s| stop_notice(&s))
}

/// chat_wait or chat_reply means the AI has seen the stop; tool calls run again.
pub(super) fn acknowledge(s: &mut Value) {
    let time = now();
    if let Some(messages) = s["messages"].as_array_mut() {
        for m in messages.iter_mut().filter(|m| {
            m["role"] == "user"
                && m["received_at"].as_u64().is_some()
                && stopped(m)
                && m["stop_acknowledged_at"].is_null()
        }) {
            m["stop_acknowledged_at"] = json!(time);
        }
    }
}

/// Late replies to a stopped message. Recalled: not persisted. Interrupted: a status
/// reply is kept so the user sees where the AI stopped.
pub(super) enum LateReply {
    Recalled,
    Interrupted,
}
pub(super) fn late_reply(s: &Value, reply_to: &str) -> Option<LateReply> {
    let messages = s["messages"].as_array()?;
    let m = messages.iter().find(|m| m["id"] == reply_to && m["role"] == "user")?;
    if m["recalled_at"].as_u64().is_some() {
        return Some(LateReply::Recalled);
    }
    // Only the most recently stopped task the AI received may get a closing status reply.
    let latest = messages.iter().rev().find(|m| {
        m["role"] == "user" && m["received_at"].as_u64().is_some() && m["interrupted_at"].as_u64().is_some()
    })?;
    (latest["id"] == reply_to).then_some(LateReply::Interrupted)
}

pub(super) const LATE_RECALLED_INSTRUCTION: &str = "The user recalled this message; the reply was not saved. Do not continue that work. Call chat_wait now for the next message.";
pub(super) const LATE_INTERRUPTED_INSTRUCTION: &str = "Saved. The user paused or replaced this task; do not continue it. Call chat_wait now for the user's next message.";

/// Recalled text never reaches the model, in excerpts or archives.
pub(super) const RECALLED_TEXT: &str = "（用户已撤回此消息）";
pub(super) fn redact(m: &mut Value) {
    if m["recalled_at"].as_u64().is_some() {
        m["text"] = json!(RECALLED_TEXT);
        if let Some(map) = m.as_object_mut() {
            map.remove("attachments");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{tool, ui as chat_ui};
    use serde_json::{json, Value};

    fn setup(root: &std::path::Path) -> (Value, Value) {
        let chat = chat_ui(root, &json!({"action":"create","seed_manual":true})).unwrap()
            ["session"]["id"]
            .clone();
        let opened = tool(root, "chat_open", &json!({"chat_id":chat})).unwrap();
        let args = json!({"chat_id":chat,"attachment_id":opened["attachment_id"]});
        (chat, args)
    }
    fn send(root: &std::path::Path, chat: &Value, id: &str, text: &str) {
        chat_ui(
            root,
            &json!({"action":"send","chat_id":chat,"message_id":id,"text":text}),
        )
        .unwrap();
    }

    #[test]
    fn recall_before_pickup_hides_the_message_from_the_ai() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (chat, args) = setup(root);
        send(root, &chat, "m1", "secret draft");
        chat_ui(
            root,
            &json!({"action":"recall","chat_id":chat,"message_id":"m1"}),
        )
        .unwrap();
        assert_eq!(tool(root, "chat_wait", &args).unwrap()["status"], "idle");
        // A delayed retry of the same send does not resurrect it.
        send(root, &chat, "m1", "secret draft");
        assert_eq!(tool(root, "chat_wait", &args).unwrap()["status"], "idle");
        let md = super::super::markdown(&super::super::load(root, chat.as_str().unwrap()).unwrap());
        assert!(!md.contains("secret draft"));
    }

    #[test]
    fn recall_during_work_stops_tools_until_chat_wait() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (chat, args) = setup(root);
        send(root, &chat, "m1", "do it");
        assert_eq!(
            tool(root, "chat_wait", &args).unwrap()["message"]["id"],
            "m1"
        );
        chat_ui(
            root,
            &json!({"action":"recall","chat_id":chat,"message_id":"m1"}),
        )
        .unwrap();
        let s = super::super::load(root, chat.as_str().unwrap()).unwrap();
        assert_eq!(
            super::stop_notice(&s).unwrap()["error"]["code"],
            "USER_RECALLED"
        );
        let late = tool(root, "chat_reply", &json!({"chat_id":chat,"attachment_id":args["attachment_id"],"message_id":"r1","reply_to":"m1","text":"done","final":true})).unwrap();
        assert_eq!(late["persisted"], false);
        let s = super::super::load(root, chat.as_str().unwrap()).unwrap();
        assert!(
            super::stop_notice(&s).is_none(),
            "chat_reply acknowledges the stop"
        );
        assert!(!s["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == "r1"));
        assert_eq!(tool(root, "chat_wait", &args).unwrap()["status"], "idle");
    }

    #[test]
    fn pause_keeps_the_connection_and_delivers_the_next_message() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (chat, args) = setup(root);
        send(root, &chat, "m1", "long task");
        tool(root, "chat_wait", &args).unwrap();
        chat_ui(root, &json!({"action":"interrupt","chat_id":chat})).unwrap();
        let s = super::super::load(root, chat.as_str().unwrap()).unwrap();
        assert_eq!(
            super::stop_notice(&s).unwrap()["error"]["code"],
            "USER_PAUSED"
        );
        let status = tool(root, "chat_reply", &json!({"chat_id":chat,"attachment_id":args["attachment_id"],"message_id":"r1","reply_to":"m1","text":"paused at step 2","final":true})).unwrap();
        assert_eq!(status["persisted"], true);
        send(root, &chat, "m2", "new instruction");
        assert_eq!(
            tool(root, "chat_wait", &args).unwrap()["message"]["id"],
            "m2"
        );
    }

    #[test]
    fn jump_queue_interrupts_and_delivers_the_correction_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (chat, args) = setup(root);
        chat_ui(
            root,
            &json!({"action":"set_queue_mode","chat_id":chat,"mode":"split"}),
        )
        .unwrap();
        send(root, &chat, "m1", "task");
        tool(root, "chat_wait", &args).unwrap();
        send(root, &chat, "q1", "later");
        send(root, &chat, "q2", "correction");
        chat_ui(
            root,
            &json!({"action":"jump_queue","chat_id":chat,"message_id":"q2"}),
        )
        .unwrap();
        let s = super::super::load(root, chat.as_str().unwrap()).unwrap();
        assert_eq!(
            super::stop_notice(&s).unwrap()["error"]["code"],
            "USER_CORRECTION"
        );
        assert_eq!(
            tool(root, "chat_wait", &args).unwrap()["message"]["id"],
            "q2"
        );
        tool(root, "chat_reply", &json!({"chat_id":chat,"attachment_id":args["attachment_id"],"message_id":"r2","reply_to":"q2","text":"ok","final":true})).unwrap();
        assert_eq!(
            tool(root, "chat_wait", &args).unwrap()["message"]["id"],
            "q1"
        );
    }

    #[test]
    fn pause_holds_the_queue_until_the_user_sends_again() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (chat, args) = setup(root);
        send(root, &chat, "m1", "task");
        tool(root, "chat_wait", &args).unwrap();
        send(root, &chat, "q1", "queued");
        assert!(chat_ui(root, &json!({"action":"interrupt","chat_id":chat,"message_id":"other"})).is_err());
        chat_ui(root, &json!({"action":"interrupt","chat_id":chat,"message_id":"m1"})).unwrap();
        assert_eq!(tool(root, "chat_wait", &args).unwrap()["status"], "idle");
        send(root, &chat, "m2", "go on");
        assert_eq!(tool(root, "chat_wait", &args).unwrap()["message"]["id"], "q1");
    }

    #[test]
    fn interrupt_needs_a_picked_up_task_and_recall_returns_merged_text() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (chat, args) = setup(root);
        send(root, &chat, "m1", "first");
        assert!(chat_ui(root, &json!({"action":"interrupt","chat_id":chat})).is_err());
        tool(root, "chat_wait", &args).unwrap();
        send(root, &chat, "q1", "a");
        send(root, &chat, "q2", "b");
        tool(root, "chat_reply", &json!({"chat_id":chat,"attachment_id":args["attachment_id"],"message_id":"r1","reply_to":"m1","text":"ok","final":true})).unwrap();
        tool(root, "chat_wait", &args).unwrap();
        let out = chat_ui(root, &json!({"action":"recall","chat_id":chat,"message_id":"q1"})).unwrap();
        let text = out["recalled"]["text"].as_str().unwrap();
        assert!(text.contains('a') && text.contains('b'), "{text}");
    }

    #[test]
    fn recalled_answer_keeps_the_question_open() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (chat, args) = setup(root);
        send(root, &chat, "m1", "task");
        tool(root, "chat_wait", &args).unwrap();
        tool(root, "chat_reply", &json!({"chat_id":chat,"attachment_id":args["attachment_id"],"message_id":"r1","reply_to":"m1","text":"which?","final":true,"awaiting_user":true})).unwrap();
        send(root, &chat, "m2", "answer");
        send(root, &chat, "q1", "unrelated");
        chat_ui(root, &json!({"action":"recall","chat_id":chat,"message_id":"m2"})).unwrap();
        let s = super::super::load(root, chat.as_str().unwrap()).unwrap();
        assert!(super::super::awaiting_confirmation(&s));
        assert_eq!(tool(root, "chat_wait", &args).unwrap()["status"], "idle");
    }

    #[test]
    fn answered_messages_cannot_be_recalled() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (chat, args) = setup(root);
        send(root, &chat, "m1", "hi");
        tool(root, "chat_wait", &args).unwrap();
        tool(root, "chat_reply", &json!({"chat_id":chat,"attachment_id":args["attachment_id"],"message_id":"r1","reply_to":"m1","text":"hello","final":true})).unwrap();
        assert!(chat_ui(
            root,
            &json!({"action":"recall","chat_id":chat,"message_id":"m1"})
        )
        .is_err());
        assert!(chat_ui(root, &json!({"action":"interrupt","chat_id":chat})).is_err());
    }
}
