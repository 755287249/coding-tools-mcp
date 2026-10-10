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
 let result=(||{fs::write(&temp,&bytes).map_err(io)?;fs::rename(&temp,safe(root,&format!("{DISCUSS_DIR}/{gid}.json"))?).map_err(io)})();let _=fs::remove_file(temp);result
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
 let posts=d["posts"].as_array().unwrap();let end=posts.len().saturating_sub(offset);let begin=end.saturating_sub(50);let mut out=d.clone();
 out["posts"]=json!(&posts[begin..end]);out["next_offset"]=if begin>0{json!(offset+end-begin)}else{Value::Null};out["total"]=json!(posts.len());
 out["member_details"]=json!(d["members"].as_array().unwrap().iter().map(|cid|match load(root,cid.as_str().unwrap_or("")){Ok(s)=>json!({"id":cid,"title":s["title"],"note":s["note"].as_str().unwrap_or(""),"status":if s["closed"]==true{"closed"}else if s["lease_until"].as_u64().unwrap_or(0)>now(){"connected"}else{"offline"}}),Err(_)=>json!({"id":cid,"title":cid,"status":"missing"})}).collect::<Vec<_>>());Ok(out)
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
 if d["posts"]!=before{write(root,d)?;}Ok(())
}
fn deliver(root:&Path,d:&Value,p:&Value)->Result<()>{
 for delivery in p["deliveries"].as_array().unwrap(){
  let mut s=load(root,id(&delivery["chat_id"])?)?;
  if s["messages"].as_array().unwrap().iter().chain(s["queue"].as_array().into_iter().flatten()).any(|m|m["id"]==delivery["message_id"]){continue;}
  if s["closed"]==true||s["archived"]==true{return Err(err("Recipient is closed or archived; existing deliveries are retained"));}
  let goal=p["goal"].as_str().filter(|s|!s.is_empty()).unwrap_or("未设置");
  let content=format!("[讨论组：{}]\n目标：{}\n发送者：{}\n用途：{}\n群 ID：{}\n任务/消息 ID：{}\n\n{}\n\n请用 chat_reply 回复本条实际消息 ID；结果会关联回讨论组。需要明确向其他成员发言时使用 chat_discuss。",d["name"].as_str().unwrap_or(""),goal,p["name"].as_str().unwrap_or(""),p["purpose"].as_str().unwrap_or(""),d["id"].as_str().unwrap_or(""),p["id"].as_str().unwrap_or(""),p["text"].as_str().unwrap_or(""));
  if !s["queue"].is_array(){s["queue"]=json!([])}
  s["queue"].as_array_mut().unwrap().push(json!({"id":delivery["message_id"],"role":"user","created_at":p["created_at"],"text":content,"discussion":{"id":d["id"],"post_id":p["id"],"source_chat_id":p["from"],"purpose":p["purpose"]}}));s["updated_at"]=json!(now());save(root,&s)?;
 }Ok(())
}
pub(super) fn action(root:&Path,args:&Value,actor:Option<&Value>)->Result<Value>{
 let action=args["action"].as_str().unwrap_or("list").trim_start_matches("discussion_");
 if action=="list"{let mut list:Vec<_>=all(root)?.into_iter().filter(|d|actor.is_none_or(|s|d["members"].as_array().unwrap().contains(&s["id"]))).map(|mut d|{d["total"]=json!(d["posts"].as_array().unwrap().len());d.as_object_mut().unwrap().remove("posts");d}).collect();list.sort_by_key(|d|std::cmp::Reverse(d["updated_at"].as_u64().unwrap_or(0)));return Ok(json!({"discussions":list}));}
 if action=="create"{
  if actor.is_some(){return Err(err("Create groups from the local interface"));}
  let gid=id(&args["discussion_id"])?;let name=text(&args["title"],160)?;let goal=if args["goal"].as_str().unwrap_or("").is_empty(){String::new()}else{text(&args["goal"],2000)?};let ids=members(root,&args["member_chat_ids"])?;
  if safe(root,&format!("{DISCUSS_DIR}/{gid}.json"))?.exists(){let old=read(root,&args["discussion_id"])?;if old["name"]!=name||old["goal"]!=goal||old["members"]!=json!(ids){return Err(err("Group ID conflicts"));}return Ok(json!({"discussion":projection(root,&old,args)?}));}
  let d=json!({"version":1,"id":gid,"name":name,"goal":goal,"members":ids,"archived":false,"created_at":now(),"updated_at":now(),"posts":[]});write(root,&d)?;return Ok(json!({"discussion":projection(root,&d,args)?}));
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
   d["updated_at"]=json!(now());write(root,&d)?;
  },
  "post"=>{
   let pid=id(&args["message_id"])?;let content=text(&args["text"],16000)?;let purpose=args["purpose"].as_str().unwrap_or("discussion");let from=actor.map(|s|s["id"].as_str().unwrap()).unwrap_or("user");
   if !["discussion","question","notice","task"].contains(&purpose){return Err(err("Invalid message purpose"));}
   let targets=args.get("recipient_chat_ids").cloned().unwrap_or_else(||json!(d["members"].as_array().unwrap().iter().filter(|v|**v!=from).cloned().collect::<Vec<_>>()));
   let targets=targets.as_array().filter(|a|!a.is_empty()&&a.len()<=16).ok_or_else(||err("Choose discussion members other than yourself"))?;
   let mut seen=HashSet::new();for target in targets{if !target.is_string()||!d["members"].as_array().unwrap().contains(target)||*target==from||!seen.insert(target.as_str()){return Err(err("Choose discussion members other than yourself"));}}
   let previous=d["posts"].as_array().unwrap().iter().find(|p|p["id"]==pid).cloned();
   let p=if let Some(p)=previous{if p["from"]!=from||p["text"]!=content||p["purpose"]!=purpose||p["targets"]!=json!(targets){return Err(err("Message ID conflicts with a discussion post"));}p}else{
    if d["archived"]==true{return Err(err("Discussion is archived"));}members(root,&json!(targets))?;
    let mut deliveries=vec![];for target in targets{let cid=target.as_str().unwrap();deliveries.push(json!({"chat_id":target,"message_id":format!("d-{}",digest(&format!("{}:{pid}:{cid}",d["id"].as_str().unwrap()))),"title":load(root,cid)?["title"],"status":"undelivered","replies":[]}));}
    let p=json!({"id":pid,"from":from,"name":actor.map(|s|s["title"].clone()).unwrap_or(json!("你")),"text":content,"goal":d["goal"],"purpose":purpose,"targets":targets,"created_at":now(),"deliveries":deliveries});d["posts"].as_array_mut().unwrap().push(p.clone());d["updated_at"]=json!(now());write(root,&d)?;p
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
   s["queue"].as_array_mut().unwrap().push(json!({"id":mid,"role":"user","created_at":now(),"text":content,"discussion":{"id":d["id"],"post_id":p["id"],"source_chat_id":delivery["chat_id"],"purpose":"result"}}));s["updated_at"]=json!(now());save(root,&s)?;
  }}
 }Ok(())
}

#[cfg(test)]
mod tests {
 use super::*;
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
