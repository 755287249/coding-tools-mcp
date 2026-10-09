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
