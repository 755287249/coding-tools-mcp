use super::{collaboration, err, id, json, now, text, Result, Value};
use std::collections::HashSet;

fn fields(v: &Value, allowed: &[&str]) -> Result<()> {
    let obj = v
        .as_object()
        .ok_or_else(|| err("Invalid question object"))?;
    if obj.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(err("Unknown question field"));
    }
    Ok(())
}
pub(super) fn parse(value: Option<&Value>) -> Result<Value> {
    let Some(value) = value else {
        return Ok(Value::Null);
    };
    let questions = value
        .as_array()
        .filter(|q| !q.is_empty() && q.len() <= 3)
        .ok_or_else(|| err("questions must contain 1–3 questions"))?;
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for q in questions {
        fields(q, &["id", "prompt", "options"])?;
        let key = id(&q["id"])?;
        if !seen.insert(key) {
            return Err(err("Duplicate question ID"));
        }
        let options = q["options"]
            .as_array()
            .filter(|o| o.len() <= 6)
            .ok_or_else(|| err("Question options must contain 0–6 choices"))?;
        let mut choices = HashSet::new();
        let mut parsed = Vec::new();
        for o in options {
            fields(o, &["id", "label", "description"])?;
            let key = id(&o["id"])?;
            if !choices.insert(key) {
                return Err(err("Duplicate option ID"));
            }
            let mut option = json!({"id":key,"label":text(&o["label"],200)?});
            if let Some(description) = o.get("description") {
                option["description"] = json!(text(description, 500)?);
            }
            parsed.push(option);
        }
        result.push(json!({"id":key,"prompt":text(&q["prompt"],1000)?,"options":parsed}));
    }
    Ok(json!(result))
}
pub(super) fn active_id(s: &Value) -> Option<String> {
    if s["closed"] == true {
        return None;
    }
    let visible = collaboration::visible(s);
    let message = visible["messages"]
        .as_array()?
        .iter()
        .rev()
        .find(|m| m["role"] == "user" || (m["role"] == "assistant" && m["final"] == true))?;
    if message["questions"]
        .as_array()
        .is_some_and(|q| !q.is_empty())
        && message["question_answer"].is_null()
    {
        message["id"].as_str().map(str::to_owned)
    } else {
        None
    }
}
pub(super) fn validate_context(
    s: &Value,
    reply_to: &str,
    final_reply: bool,
    awaiting: bool,
) -> Result<()> {
    if !final_reply || !awaiting {
        return Err(err("questions requires final=true and awaiting_user=true"));
    }
    if s["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == reply_to && m.get("discussion").is_some())
    {
        return Err(err("For discussion tasks, ask in discussion text; interactive questions require a direct conversation"));
    }
    Ok(())
}
fn answers(value: &Value, questions: &[Value]) -> Result<Value> {
    let values = value
        .as_array()
        .filter(|v| v.len() == questions.len())
        .ok_or_else(|| err("Answer every question exactly once"))?;
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for a in values {
        fields(a, &["question_id", "option_id", "custom_text"])?;
        let key = id(&a["question_id"])?;
        let q = questions
            .iter()
            .find(|q| q["id"] == key)
            .ok_or_else(|| err("Unknown question ID"))?;
        if !seen.insert(key) {
            return Err(err("Duplicate question answer"));
        }
        if a.get("option_id").is_some() == a.get("custom_text").is_some() {
            return Err(err("Choose one option or provide a custom answer"));
        }
        let answer = if let Some(option) = a.get("option_id") {
            let option = id(option)?;
            if !q["options"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["id"] == option)
            {
                return Err(err("Unknown option ID"));
            }
            json!({"question_id":key,"option_id":option})
        } else {
            json!({"question_id":key,"custom_text":text(&a["custom_text"],4000)?})
        };
        result.push(answer);
    }
    Ok(json!(questions
        .iter()
        .map(|q| result
            .iter()
            .find(|a| a["question_id"] == q["id"])
            .unwrap()
            .clone())
        .collect::<Vec<_>>()))
}
pub(super) fn answer(s: &mut Value, args: &Value) -> Result<()> {
    let card_id = id(&args["question_message_id"])?;
    let message_id = id(&args["message_id"])?;
    let index = s["messages"]
        .as_array()
        .unwrap()
        .iter()
        .position(|m| m["id"] == card_id && m["role"] == "assistant" && m["questions"].is_array())
        .ok_or_else(|| err("Question card not found"))?;
    let card = s["messages"][index].clone();
    let questions = card["questions"].as_array().unwrap();
    let answers = answers(&args["answers"], questions)?;
    if !card["question_answer"].is_null() {
        if card["question_answer"]["answers"] != answers {
            return Err(err("Question already answered differently"));
        }
        return Ok(());
    }
    if active_id(s).as_deref() != Some(card_id) {
        return Err(err("Question is no longer active; reply in the composer"));
    }
    if s["messages"]
        .as_array()
        .unwrap()
        .iter()
        .chain(s["queue"].as_array().into_iter().flatten())
        .any(|m| m["id"] == message_id)
        || s["queue_receipts"].get(message_id).is_some()
    {
        return Err(err("Message ID conflicts with an existing message"));
    }
    if s["mode"] == "group"
        && !s["members"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|m| m["id"] == card["agent_id"] && m["paused"] != true)
    {
        return Err(err("Question owner is unavailable"));
    }
    let content = questions
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let a = &answers[i];
            let answer = a["custom_text"].as_str().unwrap_or_else(|| {
                q["options"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|o| o["id"] == a["option_id"])
                    .unwrap()["label"]
                    .as_str()
                    .unwrap()
            });
            format!("{}\n{}", q["prompt"].as_str().unwrap(), answer)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut message = json!({"id":message_id,"role":"user","text":content,"attachments":[],"created_at":now(),"question_response":{"message_id":card_id,"answers":answers}});
    if s["mode"] == "group" {
        message["recipient_ids"] = json!([card["agent_id"]]);
    }
    s["messages"].as_array_mut().unwrap().push(message);
    s["messages"][index]["question_answer"] = json!({"message_id":message_id,"answers":answers});
    s["updated_at"] = json!(now());
    Ok(())
}
pub(super) fn markdown(message: &Value) -> String {
    let mut out = String::new();
    for q in message["questions"].as_array().into_iter().flatten() {
        out.push_str(&format!("\n**{}**\n", q["prompt"].as_str().unwrap_or("")));
        for o in q["options"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "- {}{}\n",
                o["label"].as_str().unwrap_or(""),
                o["description"]
                    .as_str()
                    .map(|d| format!(" — {d}"))
                    .unwrap_or_default()
            ));
        }
        out.push_str("- 自定义回答\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::{tool, ui};
    use super::*;
    fn questions() -> Value {
        json!([{"id":"scope","prompt":"范围？","options":[{"id":"custom","label":"当前页","description":"只改当前页"},{"id":"all","label":"全部"}]},{"id":"note","prompt":"补充说明？","options":[]}])
    }
    fn answers() -> Value {
        json!([{"question_id":"scope","option_id":"custom"},{"question_id":"note","custom_text":"仅修改配色 @Other"}])
    }
    #[test]
    fn durable_answers_preserve_queue_and_group_owner() {
        for grouped in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let cid = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
            let attachment = tool(
                root,
                "chat_open",
                &json!({"chat_id":cid,"agent_name":"Chief"}),
            )
            .unwrap()["attachment_id"]
                .clone();
            if grouped {
                ui(
                    root,
                    &json!({"action":"set_mode","chat_id":cid,"mode":"group"}),
                )
                .unwrap();
                tool(
                    root,
                    "chat_open",
                    &json!({"chat_id":cid,"agent_name":"Other"}),
                )
                .unwrap();
            }
            for mid in ["request", "queued"] {
                ui(
                    root,
                    &json!({"action":"send","chat_id":cid,"message_id":mid,"text":mid}),
                )
                .unwrap();
            }
            let reply = json!({"chat_id":cid,"attachment_id":attachment,"reply_to":"request","message_id":"card","text":"请选择 **范围**","questions":questions(),"final":true,"awaiting_user":true});
            tool(root, "chat_reply", &reply).unwrap();
            tool(root, "chat_reply", &reply).unwrap();
            let args = json!({"chat_id":cid,"attachment_id":attachment});
            assert_eq!(tool(root, "chat_wait", &args).unwrap()["status"], "idle");
            let read = json!({"action":"read","chat_id":cid});
            let before = ui(root, &read).unwrap()["session"].clone();
            assert_eq!(before["messages"][1]["questions_active"], true);
            let mut answer = json!({"action":"answer_question","chat_id":cid,"question_message_id":"card","message_id":"answer","answers":answers()});
            ui(root, &answer).unwrap();
            ui(root, &answer).unwrap();
            answer["message_id"] = json!("reload-retry");
            ui(root, &answer).unwrap();
            let after = ui(root, &read).unwrap()["session"].clone();
            assert_eq!(after["messages"].as_array().unwrap().len(), 3);
            assert_eq!(after["messages"][1]["questions_active"], false);
            assert_eq!(after["queued_messages"].as_array().unwrap().len(), 1);
            let delivered = tool(root, "chat_wait", &args).unwrap()["message"].clone();
            assert_eq!(delivered["id"], "answer");
            assert_eq!(
                delivered["question_response"],
                json!({"message_id":"card","answers":answers()})
            );
            if grouped {
                assert_eq!(
                    delivered["recipient_ids"],
                    json!([after["messages"][1]["agent_id"]])
                );
            }
            answer["answers"][0]["option_id"] = json!("all");
            assert!(ui(root, &answer).is_err());
            tool(root,"chat_reply",&json!({"chat_id":cid,"attachment_id":attachment,"reply_to":"answer","message_id":"done","text":"收到","final":true})).unwrap();
            assert_eq!(
                tool(root, "chat_wait", &args).unwrap()["message"]["id"],
                "queued"
            );
        }
    }
    #[test]
    fn validation_rejects_ambiguous_or_oversized_questions_and_answers() {
        for value in [
            Value::Null,
            json!([]),
            json!([questions()[0], questions()[0]]),
            json!([{"id":"q","prompt":"界".repeat(334),"options":[]}]),
            json!([{"id":"q","prompt":"?","options":[{"id":"x","label":"X"},{"id":"x","label":"Y"}]}]),
        ] {
            assert!(parse(Some(&value)).is_err());
        }
        let parsed = parse(Some(&questions())).unwrap();
        for value in [
            json!([]),
            json!([{"question_id":"scope","option_id":"bad"},{"question_id":"note","custom_text":"ok"}]),
            json!([{"question_id":"scope","option_id":"all","custom_text":"also"},{"question_id":"note","custom_text":"ok"}]),
            json!([{"question_id":"scope","option_id":"all"},{"question_id":"note","custom_text":"界".repeat(1334)}]),
        ] {
            assert!(super::answers(&value, parsed.as_array().unwrap()).is_err());
        }
        assert!(validate_context(&json!({"messages":[]}), "request", false, true).is_err());
        assert!(validate_context(&json!({"messages":[]}), "request", true, false).is_err());
        assert!(validate_context(
            &json!({"messages":[{"id":"request","discussion":{}}]}),
            "request",
            true,
            true
        )
        .is_err());
    }
    #[test]
    fn stale_closed_and_conflicting_answers_are_rejected() {
        let card = json!({"id":"card","role":"assistant","final":true,"awaiting_user":true,"questions":questions()});
        let base = json!({"messages":[card],"closed":false});
        let args = json!({"question_message_id":"card","message_id":"answer","answers":answers()});
        for closed in [false, true] {
            let mut s = base.clone();
            s["closed"] = json!(closed);
            if !closed {
                s["messages"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id":"new","role":"user","text":"new"}));
            }
            assert!(answer(&mut s, &args).is_err());
        }
        let mut s = base.clone();
        let mut conflict = args.clone();
        conflict["message_id"] = json!("card");
        assert!(answer(&mut s, &conflict).is_err());
        assert!(answer(&mut s, &args).is_ok());
    }
}
