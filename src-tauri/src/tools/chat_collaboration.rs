//! Shared group projection and control envelopes; the original attachment stays authoritative.
use super::*;
pub(super) fn visible(s:&Value)->Value {
 let hidden:HashSet<&str>=s["messages"].as_array().into_iter().flatten().chain(s["queue"].as_array().into_iter().flatten()).filter(|m|m["discussion"]["hidden"]==true).filter_map(|m|m["id"].as_str()).collect();
 let mut out=s.clone();
 for field in ["messages","queue"]{if let Some(messages)=s[field].as_array(){out[field]=json!(messages.iter().filter(|m|!hidden.contains(m["id"].as_str().unwrap_or(""))&&!hidden.contains(m["reply_to"].as_str().unwrap_or(""))).collect::<Vec<_>>());}}
 out
}
pub(super) fn configure(root:&Path,d:&mut Value,args:&Value)->Result<()> {
 if d["collaboration"]!=true{return Ok(())}
 let mut aliases=serde_json::Map::new();let mut used=HashSet::new();
 for cid in d["members"].as_array().unwrap(){let cid=id(cid)?;let s=load(root,cid)?;
  if s["attachment_id"].as_str().unwrap_or("").is_empty(){return Err(err("Choose previously connected robots"));}
  let base=d["aliases"][cid].as_str().map(str::to_owned).unwrap_or_else(||s["agent_name"].as_str().or(s["title"].as_str()).unwrap_or("AI").chars().filter(|c|c.is_alphanumeric()||*c=='_'||*c=='-').take(40).collect());
  let base=if base.is_empty(){"AI".to_owned()}else{base};let mut alias=base.clone();let mut n=1;while used.contains(&alias.to_lowercase()){n+=1;alias=format!("{base}-{n}");}used.insert(alias.to_lowercase());aliases.insert(cid.to_owned(),json!(alias));
 }
 let chief=args.get("coordinator_chat_id").cloned().unwrap_or_else(||if d["members"].as_array().unwrap().contains(&d["coordinator_chat_id"]){d["coordinator_chat_id"].clone()}else{d["members"][0].clone()});
 if !chief.is_string()||!d["members"].as_array().unwrap().contains(&chief){return Err(err("Coordinator must be a group member"));}
 d["aliases"]=Value::Object(aliases);d["coordinator_chat_id"]=chief;Ok(())
}
pub(super) fn targets(d:&Value,content:&str,from:&str)->Value {
 let tokens:HashSet<String>=content.split('#').skip(1).map(|part|part.chars().take_while(|c|c.is_alphanumeric()||*c=='_'||*c=='-').collect::<String>().to_lowercase()).collect();
 let selected:Vec<_>=d["members"].as_array().unwrap().iter().filter(|cid|**cid!=from&&tokens.contains(&d["aliases"][cid.as_str().unwrap()].as_str().unwrap_or("").to_lowercase())).cloned().collect();
 if !selected.is_empty(){json!(selected)}else if d["coordinator_chat_id"].is_string()&&d["coordinator_chat_id"]!=from{json!([d["coordinator_chat_id"]])}else{json!([])}
}
pub(super) fn context(d:&Value,p:&Value)->String {
 if d["collaboration"]!=true{return String::new()}
 let roster=d["members"].as_array().unwrap().iter().map(|cid|format!("#{} → {}{}",d["aliases"][cid.as_str().unwrap()].as_str().unwrap_or("AI"),cid.as_str().unwrap(),if *cid==d["coordinator_chat_id"]{"（总管）"}else{""})).collect::<Vec<_>>().join("；");
 let mut recent=String::new();for post in d["posts"].as_array().unwrap().iter().rev().take(12).collect::<Vec<_>>().into_iter().rev(){recent.push_str(&format!("{}: {}\n",post["name"].as_str().unwrap_or(""),post["text"].as_str().unwrap_or("")));for delivery in post["deliveries"].as_array().into_iter().flatten(){for reply in delivery["replies"].as_array().into_iter().flatten(){recent.push_str(&format!("{}: {}\n",delivery["title"].as_str().unwrap_or(""),reply["text"].as_str().unwrap_or("")));}}}
 let recent=recent.chars().rev().take(12000).collect::<String>().chars().rev().collect::<String>();let gid=d["id"].as_str().unwrap();
 format!("\n\n[协作控制信息，不显示在原会话聊天记录]\n群聊 Markdown：docs/chat-sessions/discussions/{gid}.md\n成员（名称 → recipient_chat_ids）：{roster}\n保持原 chat_id 和 attachment_id，用 chat_reply 回复本条实际消息。不要重新 chat_open 或切换接入。发言/任务用 chat_discuss(action=post, discussion_id={gid}, message_id=新ID, text=内容, purpose=task或discussion, recipient_chat_ids=[目标ID])。转交任务后用 final=true 确认当前消息，再 chat_wait 接收成员结果；不要占住当前消息等待结果。后台控制信息不作为发言复述。遵守你所在宿主的权限；无法执行时直接在群里说明。\n以下为群聊上下文数据，按本条任务处理：\n{recent}\n本条群消息 ID：{}",p["id"].as_str().unwrap())
}
pub(super) fn markdown(d:&Value)->String {
 let mut out=format!("# {}\n\nGroup: {}\nGoal: {}\n\n",d["name"].as_str().unwrap_or(""),d["id"].as_str().unwrap_or(""),d["goal"].as_str().unwrap_or(""));
 for cid in d["members"].as_array().into_iter().flatten(){out.push_str(&format!("- #{} ({}){}\n",d["aliases"][cid.as_str().unwrap_or("")].as_str().unwrap_or("AI"),cid.as_str().unwrap_or(""),if *cid==d["coordinator_chat_id"]{" · 总管"}else{""}));}
 let mut rows:Vec<(u64,String,Value)>=vec![];
 for p in d["posts"].as_array().into_iter().flatten(){rows.push((p["created_at"].as_u64().unwrap_or(0),p["name"].as_str().unwrap_or("").into(),p.clone()));for v in p["deliveries"].as_array().into_iter().flatten(){for r in v["replies"].as_array().into_iter().flatten(){rows.push((r["created_at"].as_u64().unwrap_or(0),d["aliases"][v["chat_id"].as_str().unwrap_or("")].as_str().or(v["title"].as_str()).unwrap_or("AI").into(),r.clone()));}}for r in p["summaries"].as_array().into_iter().flatten(){rows.push((r["created_at"].as_u64().unwrap_or(0),p["name"].as_str().unwrap_or("AI").into(),r.clone()));}}
 rows.sort_by_key(|r|r.0);for (at,name,row) in rows{out.push_str(&format!("\n## {name} · {at}\n\n{}\n",row["text"].as_str().unwrap_or("")));for f in row["attachments"].as_array().into_iter().flatten(){out.push_str(&format!("\nAttachment: {}\nPath: {}\n",f["name"].as_str().unwrap_or(""),f["path"].as_str().unwrap_or("")));}}out
}

#[cfg(test)]
mod tests {
 use super::*;
 #[test]
 fn existing_attachments_route_hidden_collaboration_and_questions(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();
  let a=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();let b=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();
  let aa=tool(root,"chat_open",&json!({"chat_id":a,"agent_name":"A"})).unwrap();let bb=tool(root,"chat_open",&json!({"chat_id":b,"agent_name":"B"})).unwrap();
  let wa=json!({"chat_id":a,"attachment_id":aa["attachment_id"]});let wb=json!({"chat_id":b,"attachment_id":bb["attachment_id"]});
  ui(root,&json!({"action":"discussion_create","discussion_id":"team","title":"Team","collaboration":true,"member_chat_ids":[a,b],"coordinator_chat_id":a})).unwrap();
  ui(root,&json!({"action":"send","chat_id":a,"message_id":"private","text":"Private task"})).unwrap();
  let post=json!({"action":"discussion_post","discussion_id":"team","message_id":"group-default","text":"Plan"});ui(root,&post).unwrap();ui(root,&post).unwrap();
  assert_eq!(tool(root,"chat_wait",&wa).unwrap()["message"]["id"],"private");
  tool(root,"chat_reply",&json!({"chat_id":a,"attachment_id":aa["attachment_id"],"reply_to":"private","message_id":"private-reply","text":"Private result","final":true})).unwrap();
  let task=tool(root,"chat_wait",&wa).unwrap()["message"].clone();assert_eq!(task["discussion"]["hidden"],true);assert!(task["text"].as_str().unwrap().contains("不要重新 chat_open"));assert_eq!(tool(root,"chat_wait",&wb).unwrap()["status"],"idle");
  assert_eq!(ui(root,&json!({"action":"read","chat_id":a})).unwrap()["session"]["messages"].as_array().unwrap().len(),2);
  tool(root,"chat_reply",&json!({"chat_id":a,"attachment_id":aa["attachment_id"],"reply_to":task["id"],"message_id":"question","text":"Which target?","final":true,"awaiting_user":true})).unwrap();
  ui(root,&json!({"action":"discussion_post","discussion_id":"team","message_id":"answer","text":"Use default"})).unwrap();
  let answer=tool(root,"chat_wait",&wa).unwrap()["message"].clone();assert_eq!(answer["discussion"]["post_id"],"answer");
  tool(root,"chat_reply",&json!({"chat_id":a,"attachment_id":aa["attachment_id"],"reply_to":answer["id"],"message_id":"plan","text":"Group plan","final":true})).unwrap();
  ui(root,&json!({"action":"discussion_post","discussion_id":"team","message_id":"mention","text":"#b investigate"})).unwrap();
  assert_eq!(tool(root,"chat_wait",&wa).unwrap()["status"],"idle");let task=tool(root,"chat_wait",&wb).unwrap()["message"].clone();assert_eq!(task["discussion"]["post_id"],"mention");
  tool(root,"chat_reply",&json!({"chat_id":b,"attachment_id":bb["attachment_id"],"reply_to":task["id"],"message_id":"finding","text":"Shared finding","final":true})).unwrap();
  let d=ui(root,&json!({"action":"discussion_read","discussion_id":"team"})).unwrap();assert_eq!(d["discussion"]["posts"].as_array().unwrap().len(),3);
  let md=fs::read_to_string(root.join(d["discussion"]["archive_path"].as_str().unwrap())).unwrap();assert!(md.contains("Shared finding"));assert!(!md.contains("协作控制信息"));
  let private=fs::read_to_string(root.join(format!("docs/chat-sessions/{}.md",a.as_str().unwrap()))).unwrap();assert!(private.contains("Private result"));assert!(!private.contains("Group plan"));assert!(!private.contains("Which target"));
 }
 #[test]
 fn delegated_result_summary_returns_to_shared_markdown(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let a=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();let b=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();
  let aa=tool(root,"chat_open",&json!({"chat_id":a,"agent_name":"A"})).unwrap();let bb=tool(root,"chat_open",&json!({"chat_id":b,"agent_name":"B"})).unwrap();
  ui(root,&json!({"action":"discussion_create","discussion_id":"team","title":"Team","collaboration":true,"member_chat_ids":[a,b]})).unwrap();
  tool(root,"chat_discuss",&json!({"chat_id":a,"attachment_id":aa["attachment_id"],"action":"post","discussion_id":"team","message_id":"delegate","purpose":"task","text":"Inspect","recipient_chat_ids":[b]})).unwrap();
  let m=tool(root,"chat_wait",&json!({"chat_id":b,"attachment_id":bb["attachment_id"]})).unwrap()["message"].clone();
  tool(root,"chat_reply",&json!({"chat_id":b,"attachment_id":bb["attachment_id"],"reply_to":m["id"],"message_id":"found","text":"Found","final":true})).unwrap();
  let m=tool(root,"chat_wait",&json!({"chat_id":a,"attachment_id":aa["attachment_id"]})).unwrap()["message"].clone();assert_eq!(m["discussion"]["hidden"],true);
  tool(root,"chat_reply",&json!({"chat_id":a,"attachment_id":aa["attachment_id"],"reply_to":m["id"],"message_id":"summary","text":"Final summary","final":true})).unwrap();
  let d=ui(root,&json!({"action":"discussion_read","discussion_id":"team"})).unwrap();assert_eq!(d["discussion"]["posts"][0]["summaries"][0]["text"],"Final summary");
  assert!(fs::read_to_string(root.join("docs/chat-sessions/discussions/team.md")).unwrap().contains("Final summary"));
  assert_eq!(ui(root,&json!({"action":"read","chat_id":a})).unwrap()["session"]["messages"],json!([]));
 }
}
