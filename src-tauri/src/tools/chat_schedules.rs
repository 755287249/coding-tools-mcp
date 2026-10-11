//! Durable per-folder outbox. The chat lock serializes edits, imports and delivery.
use super::*;
const STORE: &str = ".schedules";
const LIMIT: u64 = 20 * 1024 * 1024;
fn read(root: &Path) -> Result<Vec<Value>> {
    use std::io::Read;
    let path = safe(root, &format!("{DIR}/{STORE}.json"))?;
    let source = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(io(e)),
    };
    let mut bytes = Vec::new();
    source.take(LIMIT + 1).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() as u64 > LIMIT {
        return Err(err("Schedule storage exceeds limit"));
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| err(e.to_string()))?;
    if value["version"] != 1 {
        return Err(err("Invalid schedule version"));
    }
    value["tasks"]
        .as_array()
        .cloned()
        .ok_or_else(|| err("Invalid schedule storage"))
}
fn write(root: &Path, tasks: &[Value]) -> Result<()> {
    let bytes =
        serde_json::to_vec(&json!({"version":1,"tasks":tasks})).map_err(|e| err(e.to_string()))?;
    if bytes.len() as u64 > LIMIT {
        return Err(err("Schedule storage exceeds limit"));
    }
    atomic(root, STORE, "json", &bytes)
}
fn response(tasks: &[Value]) -> Value {
    json!({"tasks":tasks.iter().filter(|t|t["status"]!="deleted").collect::<Vec<_>>()})
}
pub(super) fn action(root: &Path, args: &Value) -> Result<Value> {
    let mut tasks = read(root)?;
    if args["action"] == "schedule_list" {
        return Ok(response(&tasks));
    }
    let task_id = id(&args["id"])?;
    if args["action"] == "schedule_create" || args["action"] == "schedule_import" {
        let chat_id = id(&args["chatId"])?;
        let body = message_text(&args["text"], 32000)?;
        let due = args["due"]
            .as_u64()
            .filter(|v| *v > 0 && *v <= 9007199254740991)
            .ok_or_else(|| err("Invalid schedule time"))?;
        if let Some(existing) = tasks.iter().find(|t| t["id"] == task_id) {
            if existing["status"] != "deleted"
                && (existing["chatId"] != chat_id
                    || existing["text"] != body
                    || existing["due"] != due)
            {
                return Err(err("Schedule ID conflicts with existing task"));
            }
            return Ok(response(&tasks));
        }
        if tasks.iter().filter(|t| t["status"] != "deleted").count() >= 512 {
            return Err(err("Schedule limit reached"));
        }
        let status = if args["action"] == "schedule_import" {
            match args["status"].as_str() {
                Some("pending" | "sending" | "sent" | "error" | "paused") => {
                    args["status"].as_str().unwrap()
                }
                _ => return Err(err("Invalid schedule status")),
            }
        } else {
            "pending"
        };
        if !matches!(status, "sent" | "error") && load(root, chat_id)?["closed"] == true {
            return Err(err("Conversation is closed"));
        }
        tasks.push(json!({"id":task_id,"chatId":chat_id,"title":text(&args["title"],240)?,"text":body,"due":due,"status":status}));
    } else if args["action"] == "schedule_update" {
        let task = tasks
            .iter_mut()
            .find(|t| t["id"] == task_id)
            .ok_or_else(|| err("Schedule not found"))?;
        if task["status"] == "deleted" && args["status"] == "deleted" {
            return Ok(response(&tasks));
        }
        if task["status"] != args["expected_status"] {
            return Err(err("Schedule changed; refresh before editing"));
        }
        let next = args["status"].as_str().unwrap_or("");
        let current = task["status"].as_str().unwrap_or("");
        if current == "sending"
            || current == "deleted"
            || !matches!(
                (current, next),
                ("pending", "paused") | ("paused" | "error", "pending") | (_, "deleted")
            )
        {
            return Err(err("Invalid schedule transition"));
        }
        if next == "deleted" {
            *task = json!({"id":task_id,"status":"deleted"});
        } else {
            task["status"] = json!(next);
            task.as_object_mut().unwrap().remove("error");
        }
    } else {
        return Err(err("Unknown schedule action"));
    }
    write(root, &tasks)?;
    Ok(response(&tasks))
}
pub(crate) fn tick(root: &Path) -> Result<()> {
    if !safe(root, &format!("{DIR}/{STORE}.json"))?.exists() {
        return Ok(());
    }
    let _lock = lock(root)?;
    let mut tasks = read(root)?;
    for index in 0..tasks.len() {
        let task = &tasks[index];
        if task["status"] != "sending"
            && !(task["status"] == "pending"
                && task["due"].as_u64().is_some_and(|due| due <= now()))
        {
            continue;
        }
        tasks[index]["status"] = json!("sending");
        write(root, &tasks)?;
        let result = ui_unlocked(
            root,
            &json!({"action":"send","chat_id":tasks[index]["chatId"],"message_id":tasks[index]["id"],"text":tasks[index]["text"]}),
        );
        match result {
            Ok(_) => {
                tasks[index]["status"] = json!("sent");
                tasks[index].as_object_mut().unwrap().remove("error");
            }
            Err(e) => {
                tasks[index]["status"] = json!("error");
                tasks[index]["error"] = json!(e.message());
            }
        }
        write(root, &tasks)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durable_delivery_import_retry_pause_and_tombstone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let chat = ui(root, &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let request = json!({"action":"schedule_create","id":"schedule-one","chatId":chat,"title":"task","text":"Run once","due":1});
        ui(root, &request).unwrap();
        // No page/client runner is involved; reopening the durable store recovers.
        tick(root).unwrap();
        tick(root).unwrap();
        let session = load(root, chat.as_str().unwrap()).unwrap();
        assert_eq!(session["messages"].as_array().unwrap().len(), 1);
        assert_eq!(
            ui(root, &json!({"action":"schedule_list"})).unwrap()["tasks"][0]["status"],
            "sent"
        );
        let mut imported = request.clone();
        imported["action"] = json!("schedule_import");
        imported["status"] = json!("pending");
        ui(root, &imported).unwrap();
        tick(root).unwrap();
        assert_eq!(
            load(root, chat.as_str().unwrap()).unwrap()["messages"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(ui(root,&json!({"action":"schedule_update","id":"schedule-one","expected_status":"pending","status":"paused"})).is_err());
        ui(root,&json!({"action":"schedule_update","id":"schedule-one","expected_status":"sent","status":"deleted"})).unwrap();
        ui(root, &imported).unwrap();
        assert!(
            ui(root, &json!({"action":"schedule_list"})).unwrap()["tasks"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let mut second = request.clone();
        second["id"] = json!("schedule-two");
        ui(root, &second).unwrap();
        ui(root,&json!({"action":"schedule_update","id":"schedule-two","expected_status":"pending","status":"paused"})).unwrap();
        tick(root).unwrap();
        assert_eq!(
            load(root, chat.as_str().unwrap()).unwrap()["messages"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        ui(root,&json!({"action":"schedule_update","id":"schedule-two","expected_status":"paused","status":"pending"})).unwrap();
        // Crash after delivery but before updating status: stable send ID deduplicates.
        ui(
            root,
            &json!({"action":"send","chat_id":chat,"message_id":"schedule-two","text":"Run once"}),
        )
        .unwrap();
        let mut tasks = read(root).unwrap();
        tasks[1]["status"] = json!("sending");
        write(root, &tasks).unwrap();
        tick(root).unwrap();
        let s = load(root, chat.as_str().unwrap()).unwrap();
        assert_eq!(
            s["messages"].as_array().unwrap().len()
                + s["queue"].as_array().map(Vec::len).unwrap_or(0),
            2
        );
    }
    #[test]
    fn historical_sent_schedule_migrates_without_a_live_chat() {
        let root = tempfile::tempdir().unwrap();
        ui(root.path(),&json!({"action":"schedule_import","id":"history","chatId":"removed-chat","title":"History","text":"Already delivered","due":1,"status":"sent"})).unwrap();
        tick(root.path()).unwrap();
        assert_eq!(read(root.path()).unwrap()[0]["status"], "sent");
    }
    #[test]
    fn schedules_validate_scope_payload_and_closed_chat() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let chat = ui(a.path(), &json!({"action":"create"})).unwrap()["session"]["id"].clone();
        let request = json!({"action":"schedule_create","id":"one","chatId":chat,"title":"task","text":"hi","due":1});
        assert!(ui(b.path(), &request).is_err());
        ui(a.path(), &request).unwrap();
        let mut conflict = request.clone();
        conflict["text"] = json!("different");
        assert!(ui(a.path(), &conflict).is_err());
        ui(a.path(), &json!({"action":"close","chat_id":chat})).unwrap();
        tick(a.path()).unwrap();
        assert_eq!(read(a.path()).unwrap()[0]["status"], "error");
    }
}
