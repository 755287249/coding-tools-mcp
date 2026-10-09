mod common;
use coding_tools_mcp_desktop_lib::tools::{call_tool_async,chat};
use serde_json::{json,Value};
use std::sync::Arc;
#[tokio::test]
async fn group_waits_assignments_plans_and_archives_are_isolated_by_member(){
 let root=tempfile::tempdir().unwrap();let ctx=Arc::new(common::ctx_for(root.path()));
 let cid=chat::ui(root.path(),&json!({"action":"create","mode":"group"})).unwrap()["session"]["id"].clone();
 let a=call_tool_async(ctx.clone(),"chat_open".into(),json!({"chat_id":cid,"agent_name":"Chief"})).await;
 let b=call_tool_async(ctx.clone(),"chat_open".into(),json!({"chat_id":cid,"agent_name":"Builder"})).await;
 assert_eq!(a["role"],"coordinator","{a}");assert_eq!(b["role"],"member","{b}");
 let aa=json!({"chat_id":cid,"attachment_id":a["attachment_id"],"timeout_ms":2000});let bb=json!({"chat_id":cid,"attachment_id":b["attachment_id"],"timeout_ms":2000});
 let wa=tokio::spawn({let r=root.path().to_owned();let args=aa.clone();async move{chat::wait(&r,&args).await}});
 let wb=tokio::spawn({let r=root.path().to_owned();let args=bb.clone();async move{chat::wait(&r,&args).await}});
 tokio::time::sleep(std::time::Duration::from_millis(50)).await;
 let read=||chat::ui(root.path(),&json!({"action":"read","chat_id":cid})).unwrap()["session"].clone();
 assert!(read()["members"].as_array().unwrap().iter().all(|m|m["status"]=="waiting"));
 chat::ui(root.path(),&json!({"action":"send","chat_id":cid,"message_id":"u","text":"@Builder implement"})).unwrap();
 assert_eq!(wa.await.unwrap().unwrap()["message"]["id"],"u");assert_eq!(wb.await.unwrap().unwrap()["message"]["id"],"u");assert_eq!(read()["messages"][0]["received_by"].as_array().unwrap().len(),2);
 let reply=|who:&Value,id:&str,to:&str,last:bool|json!({"chat_id":cid,"attachment_id":who["attachment_id"],"message_id":id,"reply_to":to,"text":id,"final":last});
 assert!(chat::tool(root.path(),"chat_reply",&reply(&a,"early","u",true)).is_err());
 for (who,goal) in [(&a,"Coordinate"),(&b,"Build")] {
  let result=call_tool_async(ctx.clone(),"set_todos".into(),json!({"chat_id":cid,"attachment_id":who["attachment_id"],"reply_to":"u","goal":goal,"todos":[{"id":"step","title":goal,"status":"in_progress"}]})).await;
  assert_eq!(result["persisted"],true,"{result}");
 }
 assert_eq!(read()["messages"][0]["agent_plans"].as_array().unwrap().len(),2);
 chat::tool(root.path(),"chat_reply",&reply(&b,"b-done","u",true)).unwrap();assert_eq!(read()["work_state"],"processing");
 assert!(chat::tool(root.path(),"chat_reply",&reply(&a,"b-done","u",true)).is_err());
 chat::tool(root.path(),"chat_reply",&reply(&a,"a-done","u",true)).unwrap();
 chat::ui(root.path(),&json!({"action":"send","chat_id":cid,"message_id":"next","text":"Delegate"})).unwrap();
 let mut assignment=reply(&a,"assignment","next",false);assignment["recipient_ids"]=json!([b["agent_id"]]);
 chat::tool(root.path(),"chat_reply",&assignment).unwrap();chat::tool(root.path(),"chat_reply",&assignment).unwrap();
 assert_eq!(chat::tool(root.path(),"chat_wait",&aa).unwrap()["status"],"idle");
 assert_eq!(chat::tool(root.path(),"chat_wait",&bb).unwrap()["message"]["id"],"assignment");
 assert!(chat::tool(root.path(),"chat_close",&bb).is_err());
 chat::tool(root.path(),"chat_reply",&reply(&b,"task-done","assignment",true)).unwrap();
 assert_eq!(chat::tool(root.path(),"chat_wait",&aa).unwrap()["message"]["id"],"next");
 chat::tool(root.path(),"chat_reply",&reply(&a,"all-done","next",true)).unwrap();
 let md=std::fs::read_to_string(root.path().join(read()["archive_path"].as_str().unwrap())).unwrap();assert!(md.contains("Agent: Builder"));assert!(md.contains("Coordinate"));
 for who in [&a,&b]{assert!(!md.contains(who["attachment_id"].as_str().unwrap()));assert!(!read().to_string().contains(who["attachment_id"].as_str().unwrap()));}
 assert!(chat::ui(root.path(),&json!({"action":"set_mode","chat_id":cid,"mode":"work"})).is_err());
 chat::ui(root.path(),&json!({"action":"detach_member","chat_id":cid,"member_id":b["agent_id"]})).unwrap();assert!(chat::tool(root.path(),"chat_open",&bb).is_err());
 assert_eq!(chat::ui(root.path(),&json!({"action":"set_mode","chat_id":cid,"mode":"work"})).unwrap()["session"]["mode"],"work");
 assert_eq!(chat::tool(root.path(),"chat_open",&aa).unwrap()["attachment_id"],a["attachment_id"]);
}
#[test]
fn legacy_upgrade_keeps_attachment_and_queue_targets(){
 let root=tempfile::tempdir().unwrap();let r=root.path();
 let cid=chat::ui(r,&json!({"action":"create"})).unwrap()["session"]["id"].clone();
 let a=chat::tool(r,"chat_open",&json!({"chat_id":cid,"agent_name":"Chief"})).unwrap();let aa=json!({"chat_id":cid,"attachment_id":a["attachment_id"]});
 chat::ui(r,&json!({"action":"send","chat_id":cid,"message_id":"u","text":"Work"})).unwrap();
 let progress=json!({"chat_id":cid,"attachment_id":a["attachment_id"],"message_id":"progress","reply_to":"u","text":"Working","final":false});chat::tool(r,"chat_reply",&progress).unwrap();
 chat::ui(r,&json!({"action":"set_mode","chat_id":cid,"mode":"group"})).unwrap();assert_eq!(chat::tool(r,"chat_open",&aa).unwrap()["role"],"coordinator");chat::tool(r,"chat_reply",&progress).unwrap();
 let b=chat::tool(r,"chat_open",&json!({"chat_id":cid,"agent_name":"Builder"})).unwrap();let bb=json!({"chat_id":cid,"attachment_id":b["attachment_id"]});
 chat::ui(r,&json!({"action":"send","chat_id":cid,"message_id":"q1","text":"@Builder queued"})).unwrap();chat::ui(r,&json!({"action":"send","chat_id":cid,"message_id":"q2","text":"Next"})).unwrap();
 chat::tool(r,"chat_reply",&json!({"chat_id":cid,"attachment_id":a["attachment_id"],"message_id":"done","reply_to":"u","text":"Done","final":true})).unwrap();
 chat::ui(r,&json!({"action":"rename_member","chat_id":cid,"member_id":b["agent_id"],"name":"Renamed"})).unwrap();
 assert_eq!(chat::tool(r,"chat_wait",&aa).unwrap()["message"]["id"],"q1");assert_eq!(chat::tool(r,"chat_wait",&bb).unwrap()["message"]["id"],"q1");
 assert_eq!(chat::ui(r,&json!({"action":"read","chat_id":cid})).unwrap()["session"]["queued_messages"].as_array().unwrap().len(),1);
 assert!(chat::tool(r,"chat_open",&json!({"chat_id":cid,"agent_name":"Renamed"})).is_err());
 assert!(chat::tool(r,"chat_open",&json!({"chat_id":cid,"attachment_id":"foreign"})).is_err());
}

#[tokio::test]
async fn switching_a_live_wait_preserves_presence_and_single_wait_guard(){
 let root=tempfile::tempdir().unwrap();let cid=chat::ui(root.path(),&json!({"action":"create"})).unwrap()["session"]["id"].clone();
 let aid=chat::tool(root.path(),"chat_open",&json!({"chat_id":cid})).unwrap()["attachment_id"].clone();let args=json!({"chat_id":cid,"attachment_id":aid,"timeout_ms":2000});
 let wait=tokio::spawn({let r=root.path().to_owned();let a=args.clone();async move{chat::wait(&r,&a).await}});tokio::time::sleep(std::time::Duration::from_millis(50)).await;
 chat::ui(root.path(),&json!({"action":"set_mode","chat_id":cid,"mode":"group"})).unwrap();assert_eq!(chat::ui(root.path(),&json!({"action":"read","chat_id":cid})).unwrap()["session"]["members"][0]["status"],"waiting");
 assert!(chat::wait(root.path(),&json!({"chat_id":cid,"attachment_id":aid,"timeout_ms":0})).await.is_err());
 chat::ui(root.path(),&json!({"action":"send","chat_id":cid,"message_id":"u","text":"After switch"})).unwrap();assert_eq!(wait.await.unwrap().unwrap()["message"]["id"],"u");
}

#[test]
fn group_markdown_uses_coordinator_confirmation_after_helper_final() {
 let root=tempfile::tempdir().unwrap();let r=root.path();
 let cid=chat::ui(r,&json!({"action":"create","mode":"group"})).unwrap()["session"]["id"].clone();
 let a=chat::tool(r,"chat_open",&json!({"chat_id":cid,"agent_name":"Chief"})).unwrap();
 let b=chat::tool(r,"chat_open",&json!({"chat_id":cid,"agent_name":"Builder"})).unwrap();
 chat::ui(r,&json!({"action":"send","chat_id":cid,"message_id":"u","text":"@Builder Work"})).unwrap();
 for (who,id,asking) in [(&b,"helper-done",false),(&a,"question",true)] {
  chat::tool(r,"chat_reply",&json!({"chat_id":cid,"attachment_id":who["attachment_id"],"message_id":id,"reply_to":"u","text":id,"final":true,"awaiting_user":asking})).unwrap();
 }
 let archive=r.join(format!("docs/chat-sessions/{}.md",cid.as_str().unwrap()));
 assert!(std::fs::read_to_string(&archive).unwrap().contains("消息状态：已读 · 待确认"));
 chat::ui(r,&json!({"action":"send","chat_id":cid,"message_id":"answer","text":"Approved"})).unwrap();
 assert!(std::fs::read_to_string(&archive).unwrap().contains("消息状态：已读 · 已回复"));
}
