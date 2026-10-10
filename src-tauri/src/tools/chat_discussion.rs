//! Cross-conversation groups. Called under the existing folder chat lock.
use super::*;
const DISCUSS_DIR:&str="docs/chat-sessions/discussions";
const LIMIT:usize=8*1024*1024;
fn digest(value:&str)->String {format!("{:x}",Sha256::digest(value.as_bytes()))}
fn read(root:&Path,gid:&Value)->Result<Value>{
 let gid=id(gid)?;let path=safe(root,&format!("{DISCUSS_DIR}/{gid}.json"))?;
 if fs::metadata(&path).map_err(io)?.len()>LIMIT as u64{return Err(err("Discussion archive exceeds size limit"));}
 let d:Value=serde_json::from_slice(&fs::read(path).map_err(io)?).map_err(|e|err(e.to_string()))?;
 if d["version"]!=1||d["id"]!=gid||!d["posts"].is_array()||!d["members"].is_array(){return Err(err("Invalid discussion archive"));}Ok(d)
}
fn write(root:&Path,d:&Value)->Result<()>{
 let bytes=serde_json::to_vec_pretty(d).map_err(|e|err(e.to_string()))?;
 if bytes.len()>LIMIT||d["posts"].as_array().map_or(0,Vec::len)>1000{return Err(err("Discussion is full; archive it and create another group"));}
 fs::create_dir_all(safe(root,DISCUSS_DIR)?).map_err(io)?;let gid=id(&d["id"])?;
 let temp=safe(root,&format!("{DISCUSS_DIR}/{gid}.{}.tmp",uuid::Uuid::new_v4()))?;
 let result=(||{fs::write(&temp,&bytes).map_err(io)?;fs::rename(&temp,safe(root,&format!("{DISCUSS_DIR}/{gid}.json"))?).map_err(io)})();let _=fs::remove_file(temp);result?;
 if d["collaboration"]==true{let temp=safe(root,&format!("{DISCUSS_DIR}/{gid}.{}.tmp",uuid::Uuid::new_v4()))?;let result=(||{fs::write(&temp,collaboration::markdown(d)).map_err(io)?;fs::rename(&temp,safe(root,&format!("{DISCUSS_DIR}/{gid}.md"))?).map_err(io)})();let _=fs::remove_file(temp);result?;}Ok(())
}
fn all(root:&Path)->Result<Vec<Value>>{
 let dir=safe(root,DISCUSS_DIR)?;if !dir.exists(){return Ok(vec![])}
 let mut out=vec![];for entry in fs::read_dir(dir).map_err(io)?{let p=entry.map_err(io)?.path();if p.extension().and_then(|e|e.to_str())!=Some("json"){continue}let stem=p.file_stem().and_then(|v|v.to_str()).unwrap_or("");if id(&json!(stem)).is_ok(){out.push(read(root,&json!(stem))?);}}Ok(out)
}
fn members(root:&Path,value:&Value)->Result<Vec<Value>>{
 let ids=value.as_array().filter(|a|!a.is_empty()&&a.len()<=16).ok_or_else(||err("Choose 1–16 conversations"))?;
 let mut seen=HashSet::new();for v in ids{let cid=id(v)?;if !seen.insert(cid){return Err(err("Duplicate conversation"))}let s=load(root,cid)?;if s["closed"]==true||s["archived"]==true||s["mode"]=="group"{return Err(err("Choose active independent conversations"));}}Ok(ids.clone())
}
fn projection(root:&Path,d:&Value,args:&Value)->Result<Value>{
 let offset=match args.get("offset"){None=>0,Some(v)=>v.as_u64().filter(|n|*n<=1000).ok_or_else(||err("Invalid history offset"))? as usize};
 let posts=d["posts"].as_array().unwrap();let end=posts.len().saturating_sub(offset);let begin=end.saturating_sub(50);let mut out=d.clone();out.as_object_mut().unwrap().remove("files");
 out["archive_path"]=json!(format!("{DISCUSS_DIR}/{}.md",d["id"].as_str().unwrap()));out["posts"]=json!(&posts[begin..end]);out["next_offset"]=if begin>0{json!(offset+end-begin)}else{Value::Null};out["total"]=json!(posts.len());
 out["member_details"]=json!(d["members"].as_array().unwrap().iter().map(|cid|match load(root,cid.as_str().unwrap_or("")){Ok(s)=>{let pending=pending(&s);let replies:Vec<_>=s["messages"].as_array().unwrap().iter().filter(|r|r["role"]=="assistant"&&pending.as_ref().is_some_and(|m|r["reply_to"]==m["id"])).collect();json!({"id":cid,"title":d["aliases"][cid.as_str().unwrap_or("")].as_str().or(s["agent_name"].as_str()).or(s["title"].as_str()).unwrap_or("AI"),"busy":pending.as_ref().is_some_and(|m|m["received_at"].as_u64().unwrap_or(0)>0||!replies.is_empty()),"error":replies.iter().rev().find(|r|r["tool_event"].is_object()).is_some_and(|r|r["tool_event"]["status"]=="failed"),"note":s["note"].as_str().unwrap_or(""),"status":if s["closed"]==true{"closed"}else if s["lease_until"].as_u64().unwrap_or(0)>now(){"connected"}else{"offline"}})},Err(_)=>json!({"id":cid,"title":cid,"status":"missing"})}).collect::<Vec<_>>());Ok(out)
}
fn collect(root:&Path,d:&mut Value)->Result<()>{
 let before=d["posts"].clone();
 for post in d["posts"].as_array_mut().unwrap(){for delivery in post["deliveries"].as_array_mut().unwrap(){
  if delivery["status"]=="completed"{continue}
  match load(root,delivery["chat_id"].as_str().unwrap_or("")){
   Ok(s)=>{
    let messages=s["messages"].as_array().unwrap();let empty=vec![];let queue=s["queue"].as_array().unwrap_or(&empty);
    let m=messages.iter().chain(queue.iter()).find(|m|m["id"]==delivery["message_id"]);
    let Some(m)=m else{delivery["status"]=json!(if s["closed"]==true{"closed"}else{"undelivered"});continue;};
    let replies:Vec<Value>=messages.iter().filter(|r|r["role"]=="assistant"&&r["reply_to"]==m["id"]).cloned().collect();
    let final_reply=replies.iter().rev().find(|r|r["final"]==true);
    delivery["status"]=json!(if let Some(r)=final_reply{if r["awaiting_user"]==true{"awaiting_user"}else{"completed"}}else if s["closed"]==true{"closed"}else if m["received_at"].as_u64().unwrap_or(0)>0{"processing"}else{"queued"});
    delivery["replies"]=json!(replies);delivery.as_object_mut().unwrap().remove("error");
   },Err(_)=>{delivery["status"]=json!("unavailable");delivery["error"]=json!("Conversation unavailable; existing results retained");}
  }
 }}
 if d["collaboration"]==true{let gid=d["id"].clone();for p in d["posts"].as_array_mut().unwrap(){if p["from"]=="user"{continue}if let Ok(s)=load(root,p["from"].as_str().unwrap_or("")){let ids:HashSet<_>=s["messages"].as_array().unwrap().iter().filter(|m|m["discussion"]["id"]==gid&&m["discussion"]["post_id"]==p["id"]&&m["discussion"]["purpose"]=="result").filter_map(|m|m["id"].as_str()).collect();p["summaries"]=json!(s["messages"].as_array().unwrap().iter().filter(|m|m["role"]=="assistant"&&ids.contains(m["reply_to"].as_str().unwrap_or(""))).collect::<Vec<_>>());}}}
 if d["posts"]!=before||d["collaboration"]==true{write(root,d)?;}Ok(())
}
fn deliver(root:&Path,d:&Value,p:&Value)->Result<()>{
 for delivery in p["deliveries"].as_array().unwrap(){
  let mut s=load(root,id(&delivery["chat_id"])?)?;
  if s["messages"].as_array().unwrap().iter().chain(s["queue"].as_array().into_iter().flatten()).any(|m|m["id"]==delivery["message_id"]){continue;}
  if s["closed"]==true||s["archived"]==true{return Err(err("Recipient is closed or archived; existing deliveries are retained"));}
  let goal=p["goal"].as_str().filter(|s|!s.is_empty()).unwrap_or("未设置");
  let content=format!("[讨论组：{}]\n目标：{}\n发送者：{}\n用途：{}\n群 ID：{}\n任务/消息 ID：{}\n\n{}\n\n请用 chat_reply 回复本条实际消息 ID；结果会关联回讨论组。需要明确向其他成员发言时使用 chat_discuss。",d["name"].as_str().unwrap_or(""),goal,p["name"].as_str().unwrap_or(""),p["purpose"].as_str().unwrap_or(""),d["id"].as_str().unwrap_or(""),p["id"].as_str().unwrap_or(""),p["text"].as_str().unwrap_or(""));
  if !s["queue"].is_array(){s["queue"]=json!([])}
  s["queue"].as_array_mut().unwrap().push(json!({"id":delivery["message_id"],"role":"user","created_at":p["created_at"],"attachments":p.get("attachments").cloned().unwrap_or(json!([])),"text":format!("{}{}",content,collaboration::context(d,p)),"discussion":{"hidden":d["collaboration"]==true,"id":d["id"],"post_id":p["id"],"source_chat_id":p["from"],"purpose":p["purpose"]}}));s["updated_at"]=json!(now());save(root,&s)?;
 }Ok(())
}
pub(super) fn attachment_action(root:&Path,gid:&str,args:&Value)->Result<Value>{
 let mut d=read(root,&json!(gid))?;
 let mut s=json!({"id":format!("discussion-{}",digest(gid)),"files":d.get("files").cloned().unwrap_or(json!([])),"closed":d["archived"]==true||d["paused"]==true});
 let result=match args["action"].as_str().unwrap_or(""){
  "upload"=>json!({"attachment":upload_with(root,&mut s,args,&|_|Ok(()))?}),
  "upload_chunk"=>upload_chunk_with(root,&mut s,args,&|_|Ok(()))?,
  "read_attachment_chunk"=>return read_attachment_chunk(root,&s,args),
  "read_attachment"=>{
   use base64::{engine::general_purpose::STANDARD,Engine};
   let f=message_files(&s,Some(&json!([args["upload_id"]])))?.remove(0);let target=safe(root,&file_path(&s,&f)?)?;let (hash,size,_)=fingerprint(&target)?;
   if hash!=f["sha256"].as_str().unwrap_or("")||Some(size)!=f["size"].as_u64(){return Err(err("Attachment content changed"));}
   if size>MAX_FILE_BYTES as u64{return Ok(json!({"attachment":f,"local_only":true}));}
   let bytes=fs::read(target).map_err(io)?;if super::digest(&bytes)!=hash{return Err(err("Attachment content changed"));}return Ok(json!({"attachment":f,"data_base64":STANDARD.encode(bytes)}));
  },
  "read_artifact"=>return read_artifact(root,&args["source_path"]),
  "reveal_path"=>{crate::platform::reveal::reveal_chat_path(root,args["source_path"].as_str().unwrap_or("")).map_err(err)?;return Ok(json!({"ok":true}));},
  _=>return Err(err("Unsupported group attachment action"))
 };
 d["files"]=s["files"].clone();write(root,&d)?;Ok(result)
}
pub(super) fn action(root:&Path,args:&Value,actor:Option<&Value>)->Result<Value>{
 let action=args["action"].as_str().unwrap_or("list").trim_start_matches("discussion_");
 if action=="list"{let mut list:Vec<_>=all(root)?.into_iter().filter(|d|actor.is_none_or(|s|d["members"].as_array().unwrap().contains(&s["id"]))).map(|mut d|{d["total"]=json!(d["posts"].as_array().unwrap().len());d.as_object_mut().unwrap().remove("posts");d.as_object_mut().unwrap().remove("files");d}).collect();list.sort_by_key(|d|std::cmp::Reverse(d["updated_at"].as_u64().unwrap_or(0)));return Ok(json!({"discussions":list}));}
 if action=="create"{
  if actor.is_some(){return Err(err("Create groups from the local interface"));}
  let gid=id(&args["discussion_id"])?;let name=text(&args["title"],160)?;let goal=if args["goal"].as_str().unwrap_or("").is_empty(){String::new()}else{text(&args["goal"],2000)?};let ids=members(root,&args["member_chat_ids"])?;
  if safe(root,&format!("{DISCUSS_DIR}/{gid}.json"))?.exists(){let old=read(root,&args["discussion_id"])?;if old["name"]!=name||old["goal"]!=goal||old["members"]!=json!(ids)||(old["collaboration"]==true)!=(args["collaboration"]==true)||args.get("coordinator_chat_id").is_some_and(|cid|*cid!=old["coordinator_chat_id"]){return Err(err("Group ID conflicts"));}return Ok(json!({"discussion":projection(root,&old,args)?}));}
  let mut d=json!({"collaboration":args["collaboration"]==true,"version":1,"id":gid,"name":name,"goal":goal,"members":ids,"archived":false,"created_at":now(),"updated_at":now(),"posts":[]});collaboration::configure(root,&mut d,args)?;write(root,&d)?;return Ok(json!({"discussion":projection(root,&d,args)?}));
 }
 let mut d=read(root,&args["discussion_id"])?;
 if actor.is_some_and(|s|!d["members"].as_array().unwrap().contains(&s["id"])){return Err(err("This conversation is not a discussion member"));}
 match action{
  "update"=>{
   if actor.is_some(){return Err(err("Manage groups from the local interface"));}
   if args.get("title").is_some(){d["name"]=json!(text(&args["title"],160)?);}
   if args.get("goal").is_some(){d["goal"]=json!(if args["goal"].as_str()==Some(""){String::new()}else{text(&args["goal"],2000)?});}
   if args.get("member_chat_ids").is_some(){d["members"]=json!(members(root,&args["member_chat_ids"])?);}
   if let Some(v)=args.get("archived"){if !v.is_boolean(){return Err(err("archived must be boolean"));}d["archived"]=v.clone();}
   for field in ["paused","pinned"]{if let Some(v)=args.get(field){if !v.is_boolean(){return Err(err(format!("{field} must be boolean")));}d[field]=v.clone();}}
   collaboration::configure(root,&mut d,args)?;d["updated_at"]=json!(now());write(root,&d)?;
  },
  "post"=>{
   let attachments=message_files(actor.unwrap_or(&d),args.get("attachment_ids"))?;
   let pid=id(&args["message_id"])?;let value=if args["text"].as_str().unwrap_or("").is_empty()&&!attachments.is_empty(){json!("📎")}else{args["text"].clone()};let content=text(&value,16000)?;let purpose=args["purpose"].as_str().unwrap_or("discussion");let from=actor.map(|s|s["id"].as_str().unwrap()).unwrap_or("user");
   if !["discussion","question","notice","task"].contains(&purpose){return Err(err("Invalid message purpose"));}
   let targets=args.get("recipient_chat_ids").cloned().unwrap_or_else(||if d["collaboration"]==true{collaboration::targets(&d,&content,from)}else{json!(d["members"].as_array().unwrap().iter().filter(|v|**v!=from).cloned().collect::<Vec<_>>())});
   let targets=targets.as_array().filter(|a|!a.is_empty()&&a.len()<=16).ok_or_else(||err("Choose discussion members other than yourself"))?;
   let mut seen=HashSet::new();for target in targets{if !target.is_string()||!d["members"].as_array().unwrap().contains(target)||*target==from||!seen.insert(target.as_str()){return Err(err("Choose discussion members other than yourself"));}}
   let previous=d["posts"].as_array().unwrap().iter().find(|p|p["id"]==pid).cloned();
   let p=if let Some(p)=previous{if p["from"]!=from||p["text"]!=content||p["purpose"]!=purpose||p.get("attachments").cloned().unwrap_or(json!([]))!=json!(attachments)||p["targets"]!=json!(targets){return Err(err("Message ID conflicts with a discussion post"));}p}else{
    if d["archived"]==true{return Err(err("Discussion is archived"));}if d["paused"]==true{return Err(err("Group is disconnected; reconnect from the local interface"));}members(root,&json!(targets))?;
    let mut deliveries=vec![];for target in targets{let cid=target.as_str().unwrap();deliveries.push(json!({"chat_id":target,"message_id":format!("d-{}",digest(&format!("{}:{pid}:{cid}",d["id"].as_str().unwrap()))),"title":load(root,cid)?["title"],"status":"undelivered","replies":[]}));}
    let p=json!({"id":pid,"from":from,"attachments":attachments,"attachment_chat_id":actor.map(|s|s["id"].as_str().unwrap().to_owned()).unwrap_or_else(||format!("discussion:{}",d["id"].as_str().unwrap())),"name":actor.map(|s|d["aliases"][s["id"].as_str().unwrap()].as_str().or(s["agent_name"].as_str()).map(|n|json!(n)).unwrap_or(s["title"].clone())).unwrap_or(json!("你")),"text":content,"goal":d["goal"],"purpose":purpose,"targets":targets,"created_at":now(),"deliveries":deliveries});d["posts"].as_array_mut().unwrap().push(p.clone());d["updated_at"]=json!(now());write(root,&d)?;p
   };deliver(root,&d,&p)?;
  },"read"=>(),_=>return Err(err("Unknown discussion action"))
 }
 collect(root,&mut d)?;Ok(json!({"ok":true,"persisted":true,"discussion":projection(root,&d,args)?,"post_id":args["message_id"]}))
}
pub(super) fn inbox(root:&Path,cid:&str)->Result<()>{
 for mut d in all(root)?{
  if !d["posts"].as_array().unwrap().iter().any(|p|p["from"]==cid&&p["purpose"]=="task"){continue}
  collect(root,&mut d)?;
  for p in d["posts"].as_array().unwrap().iter().filter(|p|p["from"]==cid&&p["purpose"]=="task"){for delivery in p["deliveries"].as_array().unwrap(){
   let status=delivery["status"].as_str().unwrap_or("");if !["completed","awaiting_user","closed","unavailable"].contains(&status){continue}
   let last=delivery["replies"].as_array().and_then(|a|a.last());let result_id=last.and_then(|v|v["id"].as_str()).unwrap_or(status);
   let mid=format!("r-{}",digest(&format!("{}:{}:{}:{result_id}",d["id"].as_str().unwrap(),p["id"].as_str().unwrap(),delivery["chat_id"].as_str().unwrap())));let mut s=load(root,cid)?;
   if s["closed"]==true||s["messages"].as_array().unwrap().iter().chain(s["queue"].as_array().into_iter().flatten()).any(|m|m["id"]==mid){continue}
   if !s["queue"].is_array(){s["queue"]=json!([])}
   let content=format!("[讨论组任务结果] {}\n任务 ID：{}\n成员：{}\n状态：{}\n\n{}",d["name"].as_str().unwrap(),p["id"].as_str().unwrap(),delivery["title"].as_str().unwrap(),status,last.and_then(|v|v["text"].as_str()).unwrap_or("会话不可用，请在讨论组查看状态。"));
   s["queue"].as_array_mut().unwrap().push(json!({"id":mid,"role":"user","created_at":now(),"attachments":last.and_then(|v|v.get("attachments")).cloned().unwrap_or(json!([])),"text":content,"discussion":{"hidden":d["collaboration"]==true,"id":d["id"],"post_id":p["id"],"source_chat_id":delivery["chat_id"],"purpose":"result"}}));s["updated_at"]=json!(now());save(root,&s)?;
  }}
 }Ok(())
}

#[cfg(test)]
mod tests {
 use super::*;
 #[test]
 fn group_attachments_are_independent_retryable_and_keep_original_labels(){
  use base64::{engine::general_purpose::STANDARD,Engine};
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let a=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();let b=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();
  tool(root,"chat_open",&json!({"chat_id":a,"agent_name":"A"})).unwrap();let bb=tool(root,"chat_open",&json!({"chat_id":b,"agent_name":"B"})).unwrap();
  ui(root,&json!({"action":"discussion_create","discussion_id":"files","title":"Files","member_chat_ids":[a,b],"collaboration":true,"coordinator_chat_id":a})).unwrap();
  for fid in ["before","shared"]{ui(root,&json!({"action":"upload","chat_id":b,"upload_id":fid,"name":"different.txt","data_base64":"Yg=="})).unwrap();}
  let bytes=vec![65u8;CHUNK_BYTES+17];let chunk=|offset:usize|json!({"action":"upload_chunk","chat_id":"discussion:files","upload_id":"shared","name":"large.txt","offset":offset,"total_size":bytes.len(),"data_base64":STANDARD.encode(&bytes[offset..(offset+CHUNK_BYTES).min(bytes.len())])});
  assert_eq!(ui(root,&chunk(0)).unwrap()["next_offset"],CHUNK_BYTES);assert_eq!(ui(root,&chunk(0)).unwrap()["next_offset"],CHUNK_BYTES);
  let f=ui(root,&chunk(CHUNK_BYTES)).unwrap()["attachment"].clone();assert_eq!(f["label"],"文件1");assert_eq!(ui(root,&chunk(CHUNK_BYTES)).unwrap()["attachment"],f);
  assert_eq!(ui(root,&json!({"action":"list"})).unwrap()["sessions"].as_array().unwrap().len(),2);
  let post=json!({"action":"discussion_post","discussion_id":"files","message_id":"attached","text":"","attachment_ids":["shared"],"recipient_chat_ids":[b]});
  assert_eq!(ui(root,&post).unwrap()["discussion"]["posts"][0]["text"],"📎");assert_eq!(ui(root,&post).unwrap()["discussion"]["posts"].as_array().unwrap().len(),1);
  let mut conflict=post.clone();conflict["text"]=json!("📎");conflict["attachment_ids"]=json!([]);assert!(ui(root,&conflict).is_err());
  let wait=json!({"chat_id":b,"attachment_id":bb["attachment_id"]});for _ in 0..2{assert_eq!(tool(root,"chat_wait",&wait).unwrap()["message"]["attachments"],json!([f]));}
  assert_eq!(fs::read(root.join(f["path"].as_str().unwrap())).unwrap(),bytes);
  let read=json!({"action":"read_attachment_chunk","chat_id":"discussion:files","upload_id":"shared","offset":CHUNK_BYTES});assert_eq!(STANDARD.decode(ui(root,&read).unwrap()["data_base64"].as_str().unwrap()).unwrap().len(),17);
  assert!(ui(root,&json!({"action":"discussion_read","discussion_id":"files"})).unwrap()["discussion"].get("files").is_none());assert!(fs::read_to_string(root.join("docs/chat-sessions/discussions/files.md")).unwrap().contains("large.txt"));
  ui(root,&json!({"action":"discussion_create","discussion_id":"other","title":"Other","member_chat_ids":[a,b]})).unwrap();let mut other=read.clone();other["chat_id"]=json!("discussion:other");assert!(ui(root,&other).is_err());let mut other=post.clone();other["discussion_id"]=json!("other");assert!(ui(root,&other).is_err());
  assert!(ui(root,&json!({"action":"send","chat_id":"discussion:files","message_id":"bad","text":"x"})).is_err());
  ui(root,&json!({"action":"discussion_update","discussion_id":"files","paused":true})).unwrap();assert!(ui(root,&chunk(0)).is_err());assert!(ui(root,&read).is_ok());
 }
 #[test]
 fn robot_post_and_result_inbox_retain_uploaded_files(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let a=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();let b=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();let aa=tool(root,"chat_open",&json!({"chat_id":a})).unwrap();let bb=tool(root,"chat_open",&json!({"chat_id":b})).unwrap();
  ui(root,&json!({"action":"discussion_create","discussion_id":"teamfiles","title":"Files","member_chat_ids":[a,b]})).unwrap();
  let upload=|cid:&Value,fid:&str|ui(root,&json!({"action":"upload","chat_id":cid,"upload_id":fid,"name":"file.txt","data_base64":"YQ=="})).unwrap()["attachment"].clone();let source=upload(&a,"source");let result=upload(&b,"result");
  let args=json!({"chat_id":a,"attachment_id":aa["attachment_id"],"action":"post","discussion_id":"teamfiles","message_id":"task","text":"Check","purpose":"task","recipient_chat_ids":[b],"attachment_ids":["source"]});assert_eq!(tool(root,"chat_discuss",&args).unwrap()["discussion"]["posts"][0]["attachment_chat_id"],a);
  let mut bad=args.clone();bad["message_id"]=json!("bad");bad["attachment_ids"]=json!(["result"]);assert!(tool(root,"chat_discuss",&bad).is_err());
  let received=tool(root,"chat_wait",&json!({"chat_id":b,"attachment_id":bb["attachment_id"]})).unwrap()["message"].clone();assert_eq!(received["attachments"],json!([source]));
  tool(root,"chat_reply",&json!({"chat_id":b,"attachment_id":bb["attachment_id"],"message_id":"done","reply_to":received["id"],"text":"Done","final":true,"attachment_ids":["result"]})).unwrap();assert_eq!(tool(root,"chat_wait",&json!({"chat_id":a,"attachment_id":aa["attachment_id"]})).unwrap()["message"]["attachments"],json!([result]));
 }

 #[test]
 fn offline_existing_robots_do_not_block_group_management(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let a=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();let b=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();
  tool(root,"chat_open",&json!({"chat_id":a,"agent_name":"A"})).unwrap();ui(root,&json!({"action":"discussion_create","discussion_id":"offline","title":"Team","collaboration":true,"member_chat_ids":[a]})).unwrap();ui(root,&json!({"action":"detach","chat_id":a})).unwrap();
  let changed=ui(root,&json!({"action":"discussion_update","discussion_id":"offline","paused":true,"pinned":true})).unwrap();assert_eq!(changed["discussion"]["paused"],true);
  assert_eq!(ui(root,&json!({"action":"discussion_update","discussion_id":"offline","paused":false,"member_chat_ids":[a]})).unwrap()["discussion"]["paused"],false);
  assert!(ui(root,&json!({"action":"discussion_update","discussion_id":"offline","member_chat_ids":[a,b]})).is_err());
 }
 #[test]
 fn member_presence_tracks_receipts_and_reported_errors(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let a=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();
  let aa=tool(root,"chat_open",&json!({"chat_id":a,"agent_name":"A"})).unwrap();
  ui(root,&json!({"action":"discussion_create","discussion_id":"presence-test","title":"Team","member_chat_ids":[a]})).unwrap();
  let state=||ui(root,&json!({"action":"discussion_read","discussion_id":"presence-test"})).unwrap()["discussion"]["member_details"][0].clone();
  ui(root,&json!({"action":"send","chat_id":a,"message_id":"task","text":"Check this"})).unwrap();assert_eq!(state()["busy"],false);assert_eq!(state()["status"],"connected");
  tool(root,"chat_wait",&json!({"chat_id":a,"attachment_id":aa["attachment_id"]})).unwrap();assert_eq!(state()["busy"],true);
  for (mid,status) in [("failed","failed"),("fixed","completed")]{tool(root,"chat_reply",&json!({"chat_id":a,"attachment_id":aa["attachment_id"],"reply_to":"task","message_id":mid,"text":status,"final":false,"tool_event":{"name":"test","status":status}})).unwrap();assert_eq!(state()["error"],status=="failed");}
  tool(root,"chat_reply",&json!({"chat_id":a,"attachment_id":aa["attachment_id"],"reply_to":"task","message_id":"done","text":"Done","final":true})).unwrap();assert_eq!(state()["busy"],false);assert_eq!(state()["error"],false);
  ui(root,&json!({"action":"detach","chat_id":a})).unwrap();assert_eq!(state()["status"],"offline");
 }
 #[test]
 fn group_disconnect_preserves_source_and_pending_results(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let a=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();
  let aa=tool(root,"chat_open",&json!({"chat_id":a,"agent_name":"A"})).unwrap();
  ui(root,&json!({"action":"discussion_create","discussion_id":"pause-test","title":"Team","member_chat_ids":[a],"collaboration":true})).unwrap();
  let post=json!({"action":"discussion_post","discussion_id":"pause-test","message_id":"before","text":"Finish this"});ui(root,&post).unwrap();
  let message=tool(root,"chat_wait",&json!({"chat_id":a,"attachment_id":aa["attachment_id"]})).unwrap()["message"].clone();
  ui(root,&json!({"action":"discussion_update","discussion_id":"pause-test","paused":true,"pinned":true})).unwrap();
  let mut blocked=post.clone();blocked["message_id"]=json!("blocked");assert!(ui(root,&blocked).is_err());
  assert_eq!(load(root,a.as_str().unwrap()).unwrap()["attachment_id"],aa["attachment_id"]);
  tool(root,"chat_reply",&json!({"chat_id":a,"attachment_id":aa["attachment_id"],"message_id":"result","reply_to":message["id"],"text":"Done","final":true})).unwrap();
  let read=ui(root,&json!({"action":"discussion_read","discussion_id":"pause-test"})).unwrap();assert_eq!(read["discussion"]["posts"][0]["deliveries"][0]["status"],"completed");
  ui(root,&post).unwrap();assert_eq!(read["discussion"]["posts"].as_array().unwrap().len(),1);
  ui(root,&json!({"action":"discussion_update","discussion_id":"pause-test","paused":false})).unwrap();ui(root,&blocked).unwrap();
  let list=ui(root,&json!({"action":"discussion_list"})).unwrap();assert_eq!(list["discussions"][0]["pinned"],true);assert_eq!(list["discussions"][0]["paused"],false);
  assert!(ui(root,&json!({"action":"discussion_update","discussion_id":"pause-test","paused":"true"})).is_err());
 }
 #[test]
 fn task_results_return_to_the_authenticated_caller_and_dedupe(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let a=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();let b=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();
  let aa=tool(root,"chat_open",&json!({"chat_id":a,"agent_name":"A"})).unwrap();let bb=tool(root,"chat_open",&json!({"chat_id":b,"agent_name":"B"})).unwrap();
  ui(root,&json!({"action":"discussion_create","discussion_id":"group1","title":"Team","goal":"Shared goal","member_chat_ids":[a,b]})).unwrap();
  let args=json!({"chat_id":a,"attachment_id":aa["attachment_id"],"action":"post","discussion_id":"group1","message_id":"task1","purpose":"task","text":"Investigate","recipient_chat_ids":[b]});
  tool(root,"chat_discuss",&args).unwrap();assert_eq!(tool(root,"chat_discuss",&args).unwrap()["discussion"]["posts"].as_array().unwrap().len(),1);
  let mut changed=args.clone();changed["text"]=json!("Changed");assert!(tool(root,"chat_discuss",&changed).is_err());
  let wait_b=json!({"chat_id":b,"attachment_id":bb["attachment_id"]});let msg=tool(root,"chat_wait",&wait_b).unwrap()["message"].clone();assert!(msg["text"].as_str().unwrap().contains("Shared goal"));
  tool(root,"chat_reply",&json!({"chat_id":b,"attachment_id":bb["attachment_id"],"message_id":"reply1","reply_to":msg["id"],"text":"Found it","final":true})).unwrap();
  let read=ui(root,&json!({"action":"discussion_read","discussion_id":"group1"})).unwrap();assert_eq!(read["discussion"]["posts"][0]["deliveries"][0]["status"],"completed");
  let wait_a=json!({"chat_id":a,"attachment_id":aa["attachment_id"]});let result=tool(root,"chat_wait",&wait_a).unwrap()["message"].clone();assert_eq!(result["discussion"]["purpose"],"result");assert!(result["text"].as_str().unwrap().contains("Found it"));
  tool(root,"chat_reply",&json!({"chat_id":a,"attachment_id":aa["attachment_id"],"message_id":"summary","reply_to":result["id"],"text":"Done","final":true})).unwrap();assert_eq!(tool(root,"chat_wait",&wait_a).unwrap()["status"],"idle");
  ui(root,&json!({"action":"discussion_update","discussion_id":"group1","archived":true})).unwrap();changed["message_id"]=json!("new");assert!(tool(root,"chat_discuss",&changed).is_err());
  ui(root,&json!({"action":"discussion_update","discussion_id":"group1","member_chat_ids":[b]})).unwrap();let mut removed=args.clone();removed["action"]=json!("read");assert!(tool(root,"chat_discuss",&removed).is_err());
 }
 #[test]
 fn queued_discussions_do_not_merge_with_user_messages(){
  let temp=tempfile::tempdir().unwrap();let root=temp.path();let a=ui(root,&json!({"action":"create"})).unwrap()["session"]["id"].clone();let aa=tool(root,"chat_open",&json!({"chat_id":a})).unwrap();let wait=json!({"chat_id":a,"attachment_id":aa["attachment_id"]});
  ui(root,&json!({"action":"discussion_create","discussion_id":"g","title":"T","member_chat_ids":[a]})).unwrap();
  ui(root,&json!({"action":"send","chat_id":a,"message_id":"u1","text":"first"})).unwrap();ui(root,&json!({"action":"send","chat_id":a,"message_id":"u2","text":"second"})).unwrap();
  ui(root,&json!({"action":"discussion_post","discussion_id":"g","message_id":"p1","text":"third"})).unwrap();ui(root,&json!({"action":"send","chat_id":a,"message_id":"u4","text":"fourth"})).unwrap();
  for (i,expected) in ["first","second","third","fourth"].iter().enumerate(){let m=tool(root,"chat_wait",&wait).unwrap()["message"].clone();assert!(m["text"].as_str().unwrap().contains(expected));assert!(!m["text"].as_str().unwrap().contains("队列"));tool(root,"chat_reply",&json!({"chat_id":a,"attachment_id":aa["attachment_id"],"message_id":format!("r{i}"),"reply_to":m["id"],"text":"done","final":true})).unwrap();}
 }
}
