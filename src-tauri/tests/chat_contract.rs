mod common;
use coding_tools_mcp_desktop_lib::tools::{call_tool_async, chat};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn desktop_async_dispatch_delivers_local_messages_and_saves_reply() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().to_path_buf();
    let ctx = Arc::new(common::ctx_for(&root));
    let created = chat::ui(&root, &json!({"action":"create","title":"Desktop chat"})).unwrap();
    let id = created["session"]["id"].clone();
    let opened = call_tool_async(ctx.clone(), "chat_open".into(), json!({"chat_id":id})).await;
    assert_eq!(opened["ok"], true, "{opened}");
    let attachment = opened["attachment_id"].clone();
    chat::ui(
        &root,
        &json!({"action":"send","chat_id":id,"message_id":"user-1","text":"hello"}),
    )
    .unwrap();
    let received = call_tool_async(
        ctx.clone(),
        "chat_wait".into(),
        json!({"chat_id":id,"attachment_id":attachment,"timeout_ms":0}),
    )
    .await;
    assert_eq!(received["message"]["text"], "hello", "{received}");
    let reply = call_tool_async(ctx.clone(), "chat_reply".into(), json!({"chat_id":id,"attachment_id":attachment,"message_id":"reply-1","reply_to":"user-1","text":"done","final":true})).await;
    assert_eq!(reply["persisted"], true, "{reply}");
    let read = chat::ui(&root, &json!({"action":"read","chat_id":id})).unwrap();
    assert_eq!(read["session"]["messages"][1]["text"], "done");
    let closed = call_tool_async(
        ctx,
        "chat_close".into(),
        json!({"chat_id":id,"attachment_id":attachment}),
    )
    .await;
    assert_eq!(closed["status"], "closed");
}

#[tokio::test]
async fn desktop_plan_dispatch_checks_attachment_and_persists_message_plan() {
    let root=tempfile::tempdir().unwrap();let ctx=Arc::new(common::ctx_for(root.path()));
    let cid=chat::ui(root.path(),&json!({"action":"create"})).unwrap()["session"]["id"].clone();
    chat::ui(root.path(),&json!({"action":"send","chat_id":cid,"message_id":"user","text":"work"})).unwrap();
    let aid=call_tool_async(ctx.clone(),"chat_open".into(),json!({"chat_id":cid})).await["attachment_id"].clone();
    let mut args=json!({"chat_id":cid,"attachment_id":aid,"reply_to":"user","todos":[{"id":"step","title":"Build","status":"in_progress"}]});
    let result=call_tool_async(ctx.clone(),"set_todos".into(),args.clone()).await;assert_eq!(result["persisted"],true,"{result}");
    args["attachment_id"]=json!("foreign");assert_eq!(call_tool_async(ctx.clone(),"set_todos".into(),args).await["ok"],false);
    let progress=call_tool_async(ctx.clone(),"report_progress".into(),json!({"chat_id":cid,"attachment_id":aid,"reply_to":"user","message":"Working","percent":30})).await;
    assert_eq!(progress["persisted"],true,"{progress}");
    let read=chat::ui(root.path(),&json!({"action":"read","chat_id":cid})).unwrap();assert_eq!(read["session"]["messages"][0]["task_plan"]["progress"]["percent"],30);
    let legacy=call_tool_async(ctx,"set_todos".into(),json!({"goal":"legacy","todos":[{"id":"legacy","title":"Legacy task","status":"pending"}]})).await;assert_eq!(legacy["ok"],true,"{legacy}");
    let after=chat::ui(root.path(),&json!({"action":"read","chat_id":cid})).unwrap();assert_eq!(after["session"]["messages"][0]["task_plan"],read["session"]["messages"][0]["task_plan"]);
}

fn guidance(name: &str, args: &serde_json::Value, result: serde_json::Value) -> String {
    let wrapped = coding_tools_mcp_desktop_lib::tools::wrap_mcp_tool_result(name, args, result);
    assert_eq!(wrapped["isError"], false);
    let text = wrapped["content"][0]["text"].as_str().unwrap();
    assert_eq!(text, wrapped["structuredContent"]["instruction"]);
    assert!(text.len() < 400);
    text.to_owned()
}

#[tokio::test]
async fn chat_guidance_distinguishes_progress_final_idle_and_closed() {
    for mode in ["work", "group"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cid = chat::ui(root, &json!({"action":"create","mode":mode})).unwrap()["session"]["id"].clone();
        let opened = chat::tool(root, "chat_open", &json!({"chat_id":cid,"agent_name":"Test"})).unwrap();
        let args = json!({"chat_id":cid,"attachment_id":opened["attachment_id"]});
        let open_text = guidance("chat_open", &args, opened.clone());
        assert!(open_text.contains("complete skill.text"));
        assert!(open_text.contains("25000"));
        let resumed = chat::tool(root, "chat_open", &args).unwrap();
        assert_eq!(resumed["attachment_id"], opened["attachment_id"]);
        assert_eq!(guidance("chat_open", &args, resumed), open_text);
        let idle_text = guidance("chat_wait", &args, chat::tool(root, "chat_wait", &args).unwrap());
        assert!(idle_text.contains("Idle is not an exit"));
        for timeout in [0, 1] {
            let mut wait_args = args.clone(); wait_args["timeout_ms"] = json!(timeout);
            assert_eq!(guidance("chat_wait", &wait_args, chat::wait(root, &wait_args).await.unwrap()), idle_text);
        }
        chat::ui(root, &json!({"action":"send","chat_id":cid,"message_id":"user","text":"PRIVATE_BODY"})).unwrap();
        let received = chat::tool(root, "chat_wait", &args).unwrap();
        assert_eq!(received["message"]["id"], "user");
        let message_text = guidance("chat_wait", &args, received);
        assert!(message_text.contains("message.id as reply_to"));
        assert!(!message_text.contains("PRIVATE_BODY"));
        let mut reply = json!({"chat_id":cid,"attachment_id":args["attachment_id"],"reply_to":"user","message_id":"progress","text":"Working","final":false});
        let progress = chat::tool(root, "chat_reply", &reply).unwrap();
        assert_eq!(progress["persisted"], true);
        assert_eq!(chat::tool(root, "chat_reply", &reply).unwrap(), progress);
        let progress_text = guidance("chat_reply", &reply, progress);
        assert!(progress_text.contains("Continue the current task"));
        assert!(!progress_text.contains("chat_wait"));
        assert_eq!(chat::tool(root, "chat_wait", &args).unwrap()["message"]["id"], "user");
        reply["message_id"] = json!("final"); reply.as_object_mut().unwrap().remove("final");
        let final_result = chat::tool(root, "chat_reply", &reply).unwrap();
        assert_eq!(final_result["persisted"], true);
        assert_eq!(chat::tool(root, "chat_reply", &reply).unwrap(), final_result);
        let final_text = guidance("chat_reply", &reply, final_result);
        assert!(final_text.contains("does not close the chat"));
        assert!(final_text.contains("chat_wait(timeout_ms:25000)"));
        reply["text"] = json!("Changed");
        assert!(chat::tool(root, "chat_reply", &reply).is_err());
        if mode == "work" {
            chat::ui(root, &json!({"action":"send","chat_id":cid,"message_id":"question-task","text":"Choose"})).unwrap();
            chat::tool(root, "chat_wait", &args).unwrap();
            let question = json!({"chat_id":cid,"attachment_id":args["attachment_id"],"reply_to":"question-task","message_id":"question","text":"Which?","final":true,"awaiting_user":true,"questions":[{"id":"choice","prompt":"Which?","options":[]}]});
            let saved = chat::tool(root, "chat_reply", &question).unwrap();
            assert_eq!(saved["persisted"], true);
            assert_eq!(guidance("chat_reply", &question, saved), final_text);
        }
        let mut wait_args = args.clone(); wait_args["timeout_ms"] = json!(1000);
        let (closed_wait, closed) = tokio::join!(chat::wait(root, &wait_args), async {
            tokio::task::yield_now().await;
            chat::tool(root, "chat_close", &args).unwrap()
        });
        let closed_text = guidance("chat_close", &args, closed);
        assert!(closed_text.contains("Stop waiting"));
        assert_eq!(guidance("chat_wait", &args, closed_wait.unwrap()), closed_text);
        for name in ["chat_open", "chat_wait", "chat_reply", "chat_close"] {
            let result = chat::tool(root, name, &args).unwrap();
            assert!(result.get("persisted").is_none());
            assert_eq!(guidance(name, &args, result), closed_text);
        }
    }
}

#[test]
fn chat_error_text_does_not_claim_a_successful_reply() {
    let wrapped = coding_tools_mcp_desktop_lib::tools::wrap_mcp_tool_result("chat_reply", &json!({}),
        json!({"ok":false,"error":{"message":"Storage unavailable"},"instruction":"Final reply persisted"}));
    assert_eq!(wrapped["isError"], true);
    assert_eq!(wrapped["content"][0]["text"], "Storage unavailable");
}
