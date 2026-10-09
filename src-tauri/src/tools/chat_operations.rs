//! Actual MCP execution evidence, scoped by transport identity + root + delivered request.
//! Sidecar archives do not consume the chat message budget or expose attachment tokens.
use super::*;
use std::collections::HashMap;

#[derive(Clone)]
struct Binding { chat_id:String, attachment_id:String, reply_to:Option<String> }
static BINDINGS:OnceLock<Mutex<HashMap<(PathBuf,String),Vec<Binding>>>>=OnceLock::new();
fn clean(v:&Value,max:usize)->String {
 let text=v.as_str().map(str::to_owned).unwrap_or_else(||v.to_string());
 super::super::redaction::redact_sensitive_text(&text).0.chars().take(max).collect()
}
fn owner(root:&Path,b:&Binding,request:bool)->Option<Value>{
 let s=load(root,&b.chat_id).ok()?;if s["closed"]==true{return None;}
 let args=json!({"attachment_id":b.attachment_id});owned(&s,&args).ok()?;
 let actor=if group::grouped(&s){Some(group::member_for(&s,&args,false).ok()?)}else{None};
 if request {
  let reply=b.reply_to.as_ref()?;let messages=s["messages"].as_array()?;let m=messages.iter().find(|m|m["id"]==*reply)?;
  if m["kind"]=="connection_request"{return None;}
  let received=if let Some(a)=&actor{m["received_by"].as_array().is_some_and(|ids|ids.contains(&a["id"]))}else{m["received_at"].as_u64().is_some()};
  if !received||messages.iter().any(|r|r["reply_to"]==*reply&&r["final"]==true&&actor.as_ref().is_none_or(|a|r["agent_id"]==a["id"])){return None;}
 }
 Some(if let Some(a)=actor{json!({"agent_id":a["id"],"agent_name":a["name"]})}else{json!({"agent_name":s["agent_name"].as_str().unwrap_or("AI")})})
}
pub fn bind(root:&Path,key:Option<&str>,name:&str,args:&Value,result:&Value){
 let Some(key)=key else{return};if result["ok"]!=true||!matches!(name,"chat_open"|"chat_wait"|"chat_close"){return;}
 let Some(chat_id)=args["chat_id"].as_str()else{return};
 let attachment=result["attachment_id"].as_str().or_else(||args["attachment_id"].as_str()).unwrap_or("");
 let Ok(mut map)=BINDINGS.get_or_init(Default::default).lock()else{return};
 let k=(root.to_path_buf(),key.to_owned());
 // Bounded registry: evicting an entry disables attribution until the next wait.
 if map.len()>=256&&!map.contains_key(&k){map.clear();}
 let entries=map.entry(k).or_default();entries.retain(|b|owner(root,b,false).is_some()&&(name!="chat_close"||b.chat_id!=chat_id));
 if name=="chat_close"||attachment.is_empty(){return;}
 if !entries.iter().any(|b|b.chat_id==chat_id&&b.attachment_id==attachment){entries.push(Binding{chat_id:chat_id.into(),attachment_id:attachment.into(),reply_to:None});}
 let b=entries.iter_mut().find(|b|b.chat_id==chat_id&&b.attachment_id==attachment).unwrap();
 if name=="chat_wait"{b.reply_to=if result["status"]=="message"{result["message"]["id"].as_str().map(str::to_owned)}else{None};}
}
fn add_path(paths:&mut Vec<String>,v:&Value){if let Some(p)=v.as_str().filter(|p|!p.trim().is_empty()){let p=clean(&json!(p),1000);if paths.len()<24&&!paths.contains(&p){paths.push(p);}}}
fn paths(value:&Value,out:&mut Vec<String>,depth:usize){
 if depth>5||out.len()>=24{return;}
 match value{
  Value::Array(items)=>for item in items{paths(item,out,depth+1)},
  Value::Object(map)=>for(k,v)in map{match k.as_str(){
   "path"|"destination"|"source"|"file"|"workdir"|"cwd"=>add_path(out,v),
   "paths"|"files"|"files_changed"=>if let Some(items)=v.as_array(){for item in items{add_path(out,item);paths(item,out,depth+1);}},
   "patch"=>if let Some(patch)=v.as_str(){for line in patch.lines(){for prefix in ["*** Update File: ","*** Add File: ","*** Delete File: ","*** Move to: ","+++ b/"]{if let Some(p)=line.strip_prefix(prefix){add_path(out,&json!(p));}}}},
   "content"|"text"|"output"|"stdout"|"stderr"|"diff"=>{},
   _=>paths(v,out,depth+1)
  }},_=>{}
 }
}
fn kind(tool:&str)->&'static str{
 if tool.starts_with("read_")||matches!(tool,"list_files"|"view_image"){"read"}
 else if ["search","grep","glob"].iter().any(|s|tool.contains(s)){"search"}
 else if ["file_ops","patch","edit","write","format_files"].iter().any(|s|tool.contains(s)){"edit"}
 else if ["exec","command","send_input","kill_session"].iter().any(|s|tool.contains(s)){"exec"}else{"other"}
}
fn fields(value:&Value,keys:&[&str])->Value{let mut out=json!({});for key in keys{if let Some(v)=value.get(*key){out[*key]=v.clone();}}out}
fn read(root:&Path,chat_id:&str)->Result<Vec<Value>>{
 let target=safe(root,&format!("{DIR}/{}.operations.json",id(&json!(chat_id))?))?;
 if !target.exists(){return Ok(vec![]);}
 if fs::metadata(&target).map_err(io)?.len()>600000{return Err(err("Operation archive exceeds limit"));}
 serde_json::from_slice(&fs::read(target).map_err(io)?).map_err(|_|err("Invalid operation archive"))
}
pub fn view(root:&Path,chat_id:&str)->Value{
 match read(root,chat_id){Ok(events)=>json!({"operations":events}),Err(_)=>json!({"operations":[],"operations_error":"Operation archive unavailable"})}
}
fn write(root:&Path,chat_id:&str,event:&Value)->Result<()>{
 let _lock=lock(root)?;let mut events=read(root,chat_id)?;
 if let Some(index)=events.iter().position(|e|e["id"]==event["id"]){events[index]=event.clone();}else{events.push(event.clone());}
 while events.len()>240||serde_json::to_vec(&events).unwrap().len()>512000{events.remove(0);}
 atomic(root,chat_id,"operations.json",&serde_json::to_vec(&events).unwrap())?;
 let mut md=String::from("# MCP operations\n\nBounded recent operation history; running is not proof of completion.\n\n");
 for e in events{md.push_str(&format!("## {} · {} · {}\n\nRequest: {}\nTime: {}\n{}\n{}\n{}\n{}\n{}\n{}\n",e["agent_name"].as_str().unwrap_or("AI"),e["tool"].as_str().unwrap_or(""),e["status"].as_str().unwrap_or(""),e["reply_to"].as_str().unwrap_or(""),e["started_at"],e["paths"].as_array().map(|a|a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default(),if e["dry_run"]==true{"Dry run"}else{""},e["input"].as_str().unwrap_or(""),e["output"].as_str().unwrap_or(""),e["diff"].as_str().unwrap_or(""),if e["truncated"]==true{"Details truncated"}else{""}));}
 atomic(root,chat_id,"operations.md",md.as_bytes())
}
pub struct Operation {root:PathBuf,chat_id:String,event:Value,finished:bool,warning:bool}
pub fn begin(root:&Path,key:Option<&str>,tool:&str,args:&Value)->Option<Operation>{
 let key=key?;if tool.starts_with("chat_")||matches!(tool,"set_todos"|"update_plan"|"report_progress"){return None;}
 let mut map=BINDINGS.get_or_init(Default::default).lock().ok()?;
 let entries=map.get_mut(&(root.to_path_buf(),key.into()))?;entries.retain(|b|owner(root,b,false).is_some());
 if entries.len()!=1{return None;}
 let b=entries[0].clone();let actor=owner(root,&b,true)?;drop(map);
 let mut found=vec![];paths(args,&mut found,0);
 let input=fields(args,&["cmd","command","program","args","script","pattern","query","start_line","end_line","dry_run"]);
 let mut event=json!({"id":uuid::Uuid::new_v4().to_string(),"reply_to":b.reply_to,"tool":tool,"kind":kind(tool),"status":"running","started_at":now(),"paths":found,"input":if input==json!({}){String::new()}else{clean(&input,2000)},"dry_run":args["dry_run"]==true});
 event.as_object_mut()?.extend(actor.as_object()?.clone());
 let warning=write(root,&b.chat_id,&event).is_err();
 Some(Operation{root:root.to_path_buf(),chat_id:b.chat_id,event,finished:false,warning})
}
impl Operation{
 pub fn finish(&mut self,result:&Value,interrupted:bool)->Option<&'static str>{
  self.finished=true;self.event["duration_ms"]=json!(now().saturating_sub(self.event["started_at"].as_u64().unwrap_or(now())));
  self.event["status"]=json!(if interrupted{"interrupted"}else if result["ok"]==false||result["isError"]==true||result["exit_code"].as_i64().is_some_and(|n|n!=0){"failed"}else if matches!(result["status"].as_str(),Some("running"|"background")){"running"}else{"completed"});
  if result["dry_run"]==true{self.event["dry_run"]=json!(true);}
  let mut found:Vec<String>=serde_json::from_value(self.event["paths"].clone()).unwrap_or_default();paths(&fields(result,&["affected_files","files_changed"]),&mut found,0);self.event["paths"]=json!(found);
  let summary=fields(result,&["status","exit_code","error","message","bytes_read","total_lines","affected_files","files_changed","session_id","operation_id","stdout","stderr","output"]);
  self.event["output"]=json!(if summary==json!({}){String::new()}else{clean(&summary,4000)});
  self.event["truncated"]=json!(summary.to_string().chars().count()>4000);
  if let Some(diff)=result["diff"].as_str(){self.event["diff"]=json!(clean(&json!(diff),12000));if diff.chars().count()>12000||result["diff_truncated"]==true{self.event["truncated"]=json!(true);}}
  self.warning|=write(&self.root,&self.chat_id,&self.event).is_err();
  self.warning.then_some("Operation log could not be saved; do not retry the tool solely for this warning.")
 }
}
impl Drop for Operation{fn drop(&mut self){if !self.finished{self.finish(&json!({}),true);}}}

#[cfg(test)]
mod tests {
 use super::*;
 fn attached(root:&Path,key:&str,mode:&str)->Value{
  let cid=super::super::ui(root,&json!({"action":"create","mode":mode})).unwrap()["session"]["id"].clone();
  let open=json!({"chat_id":cid,"agent_name":"Builder"});let result=tool(root,"chat_open",&open).unwrap();bind(root,Some(key),"chat_open",&open,&result);
  let args=json!({"chat_id":cid,"attachment_id":result["attachment_id"],"timeout_ms":0});
  super::super::ui(root,&json!({"action":"send","chat_id":cid,"message_id":"u","text":"inspect"})).unwrap();
  let message=tool(root,"chat_wait",&args).unwrap();bind(root,Some(key),"chat_wait",&args,&message);args
 }
 #[test]
 fn operations_preserve_actor_request_and_actual_results_without_chat_tokens(){
  let root=tempfile::tempdir().unwrap();let args=attached(root.path(),"parallel","group");
  let mut a=begin(root.path(),Some("parallel"),"file_ops",&json!({"operations":[{"path":"a.txt","content":"private-body"}],"dry_run":true})).unwrap();
  let mut b=begin(root.path(),Some("parallel"),"exec_command",&json!({"command":"echo password=synthetic-secret"})).unwrap();
  b.finish(&json!({"ok":true,"exit_code":2,"stdout":"token=synthetic-token"}),false);
  a.finish(&json!({"ok":true,"diff":"--- a/a.txt\n+++ b/a.txt\n+password=hidden-value\n","affected_files":[{"path":"a.txt","operation":"write"}]}),false);
  let events=read(root.path(),args["chat_id"].as_str().unwrap()).unwrap();assert_eq!(events.len(),2);assert_eq!(events[0]["agent_name"],"Builder");assert!(events[0]["agent_id"].is_string());assert_eq!(events[0]["reply_to"],"u");assert_eq!(events[0]["paths"],json!(["a.txt"]));assert_eq!(events[0]["dry_run"],true);assert_eq!(events[1]["status"],"failed");
  let serialized=serde_json::to_string(&events).unwrap();for secret in ["private-body","synthetic-secret","synthetic-token","hidden-value",args["attachment_id"].as_str().unwrap()]{assert!(!serialized.contains(secret));}
  let dropped=begin(root.path(),Some("parallel"),"exec_command",&json!({})).unwrap();drop(dropped);assert_eq!(read(root.path(),args["chat_id"].as_str().unwrap()).unwrap().last().unwrap()["status"],"interrupted");
  let md=fs::read_to_string(root.path().join(format!("docs/chat-sessions/{}.operations.md",args["chat_id"].as_str().unwrap()))).unwrap();assert!(md.contains("Builder · file_ops"));
 }
 #[test]
 fn ambiguous_identities_expired_owners_and_wrong_roots_do_not_leak(){
  let root=tempfile::tempdir().unwrap();let other=tempfile::tempdir().unwrap();let a=attached(root.path(),"shared","work");
  assert!(begin(root.path(),Some("foreign"),"read_file",&json!({})).is_none());assert!(begin(other.path(),Some("shared"),"read_file",&json!({})).is_none());
  let b=attached(root.path(),"shared","work");assert!(begin(root.path(),Some("shared"),"read_file",&json!({})).is_none());
  super::super::ui(root.path(),&json!({"action":"close","chat_id":b["chat_id"]})).unwrap();
  let mut op=begin(root.path(),Some("shared"),"read_file",&json!({"path":"a.txt"})).unwrap();op.finish(&json!({"ok":true,"content":"private-file-body","bytes_read":42}),false);
  assert_eq!(read(root.path(),b["chat_id"].as_str().unwrap()).unwrap().len(),0);
  let mut reply=a.clone();reply["reply_to"]=json!("u");reply["message_id"]=json!("done");reply["text"]=json!("done");reply["final"]=json!(true);tool(root.path(),"chat_reply",&reply).unwrap();
  assert!(begin(root.path(),Some("shared"),"read_file",&json!({})).is_none());
  assert!(!read(root.path(),a["chat_id"].as_str().unwrap()).unwrap()[0].to_string().contains("private-file-body"));
 }
 #[test]
 fn bounded_archive_preserves_messages_and_reports_corruption(){
  let root=tempfile::tempdir().unwrap();let args=attached(root.path(),"bounded","work");let cid=args["chat_id"].as_str().unwrap();
  let mut op=begin(root.path(),Some("bounded"),"read_file",&json!({"path":"a"})).unwrap();op.finish(&json!({"ok":true}),false);
  let mut event=read(root.path(),cid).unwrap()[0].clone();event["output"]=json!("x".repeat(4000));
  for i in 0..250{event["id"]=json!(i.to_string());write(root.path(),cid,&event).unwrap();}
  let events=read(root.path(),cid).unwrap();assert!(events.len()<=240);assert!(serde_json::to_vec(&events).unwrap().len()<=512000);assert_eq!(events.last().unwrap()["id"],"249");assert_eq!(load(root.path(),cid).unwrap()["messages"].as_array().unwrap().len(),1);
  let target=root.path().join(format!("docs/chat-sessions/{cid}.operations.json"));fs::write(&target,"broken").unwrap();assert!(view(root.path(),cid)["operations_error"].is_string());assert!(write(root.path(),cid,&event).is_err());assert_eq!(fs::read_to_string(target).unwrap(),"broken");
 }
}

