//! Participant leases and addressed tasks. Caller holds the chat storage lock.
use super::*;
pub(super) fn grouped(s:&Value)->bool{s["mode"]=="group"}
pub(super) fn members(s:&Value)->Vec<Value>{s["members"].as_array().cloned().unwrap_or_default()}
pub(super) fn member_name(v:&Value)->Result<String>{
 let name=text(v,80)?;
 if name.chars().any(|c|c.is_whitespace()||c.is_control()||"@[]<>".contains(c)){return Err(err("Agent name must be 1–80 bytes without spaces, @ or markup"));}Ok(name)
}
pub(super) fn member_for(s:&Value,args:&Value,expired:bool)->Result<Value>{
 let member=members(s).into_iter().find(|m|args["attachment_id"].as_str().is_some_and(|v|!v.is_empty())&&m["attachment_id"]==args["attachment_id"]).ok_or_else(||err("Chat attachment expired; call chat_open again with the saved attachment_id"))?;
 if member["paused"]==true{return Err(err("Group member is paused; resume it in the local UI"));}
 let _=expired;// Keepalive: a member's own attachment_id stays valid after the lease lapses.
 Ok(member)
}
pub(super) fn renew(s:&mut Value,args:&Value)->Result<()>{
 if grouped(s){let m=member_for(s,args,false)?;for p in s["members"].as_array_mut().unwrap(){if p["id"]==m["id"]{p["lease_until"]=json!(now()+LEASE_MS);}}}else{s["lease_until"]=json!(now()+LEASE_MS);}Ok(())
}
pub(super) fn open(s:&mut Value,args:&Value)->Result<Value>{
 if args["attachment_id"].as_str().is_some_and(|v|!v.is_empty()){
  let mut m=member_for(s,args,true)?;m["lease_until"]=json!(now()+LEASE_MS);for p in s["members"].as_array_mut().unwrap(){if p["id"]==m["id"]{*p=m.clone();}}return Ok(m);
 }
 let name=member_name(&args["agent_name"])?;let all=members(s);
 if all.iter().any(|m|m["name"]==name){return Err(err("Agent name already exists; resume with its saved attachment_id or choose another name"));}
 if all.len()>=16{return Err(err("Group member limit is 16"));}
 let m=json!({"id":uuid::Uuid::new_v4().to_string(),"name":name,"role":if all.is_empty(){"coordinator"}else{"member"},"attachment_id":uuid::Uuid::new_v4().to_string(),"lease_until":now()+LEASE_MS});
 if s["members"].is_null(){s["members"]=json!([]);}s["members"].as_array_mut().unwrap().push(m.clone());Ok(m)
}
pub(super) fn complete(s:&Value,m:&Value)->bool{
 let replies:Vec<_>=s["messages"].as_array().unwrap().iter().filter(|r|r["role"]=="assistant"&&r["reply_to"]==m["id"]&&r["final"]==true).collect();
 if let Some(ids)=m["recipient_ids"].as_array().filter(|a|!a.is_empty()){ids.iter().all(|id|replies.iter().any(|r|r["agent_id"]==*id))}else{!replies.is_empty()}
}
pub(super) fn pending_for(s:&Value,agent:Option<&Value>,waiting:bool)->Option<Value>{
 s["messages"].as_array()?.iter().find(|m|{
  if let Some(agent)=agent{
   if waiting&&m["role"]=="user"&&members(s).iter().any(|p|p["id"]==*agent&&p["role"]=="coordinator")&&s["messages"].as_array().unwrap().iter().any(|child|child["kind"]=="assignment"&&child["reply_to"]==m["id"]&&!complete(s,child)){return false;}
   let ids=m["recipient_ids"].as_array().cloned().unwrap_or_else(||members(s).iter().filter(|p|p["role"]=="coordinator").map(|p|p["id"].clone()).collect());
   (m["role"]=="user"||m["kind"]=="assignment")&&ids.contains(agent)&&!s["messages"].as_array().unwrap().iter().any(|r|r["role"]=="assistant"&&r["reply_to"]==m["id"]&&r["agent_id"]==*agent&&r["final"]==true)&&!(m["recipient_ids"].is_null()&&complete(s,m))
  }else{m["role"]=="user"&&!complete(s,m)}
 }).cloned()
}
pub(super) fn target_user(s:&Value,m:&mut Value){
 if !grouped(s){return;}let all=members(s);let Some(chief)=all.iter().find(|p|p["role"]=="coordinator") else{return};
 let tokens:Vec<_>=m["text"].as_str().unwrap_or("").split_whitespace().collect();
 let mut ids=vec![chief["id"].clone()];for p in &all{if p["id"]!=chief["id"]&&tokens.contains(&format!("@{}",p["name"].as_str().unwrap_or("")).as_str()){ids.push(p["id"].clone());}}
 m["recipient_ids"]=json!(ids);
}
pub(super) fn bind_targets(s:&mut Value){
 if !grouped(s){return;}let snapshot=s.clone();for m in s["messages"].as_array_mut().unwrap(){if m["role"]=="user"&&m["recipient_ids"].is_null()&&!complete(&snapshot,m){target_user(&snapshot,m);}}
}
pub(super) fn reply_identity(s:&Value,args:&Value,final_reply:bool)->Result<Value>{
 if !grouped(s){if args.get("recipient_ids").is_some(){return Err(err("Assignments require group mode"));}return Ok(json!({}));}
 let actor=member_for(s,args,false)?;let mut result=json!({"agent_id":actor["id"],"agent_name":actor["name"]});
 if let Some(value)=args.get("recipient_ids"){
  let ids=value.as_array().filter(|a|!a.is_empty()).ok_or_else(||err("Select unique other members with final=false"))?;
  let unique:HashSet<_>=ids.iter().filter_map(Value::as_str).collect();
  if actor["role"]!="coordinator"||final_reply||unique.len()!=ids.len()||ids.iter().any(|id|*id==actor["id"]||!members(s).iter().any(|m|m["id"]==*id)){return Err(err("Only the coordinator can assign unique other members with final=false"));}
  result["recipient_ids"]=value.clone();result["kind"]=json!("assignment");
 }Ok(result)
}
pub(super) fn validate_final(s:&Value,args:&Value)->Result<()>{
 if !grouped(s)||args["final"]==false{return Ok(());}let actor=member_for(s,args,false)?;
 if args["awaiting_user"]==true&&actor["role"]!="coordinator"{return Err(err("Members should report questions to the coordinator"));}
 if actor["role"]=="coordinator"{
  let messages=s["messages"].as_array().unwrap();let parent=messages.iter().find(|m|m["id"]==args["reply_to"]);
  let unfinished=messages.iter().any(|m|m["kind"]=="assignment"&&m["reply_to"]==args["reply_to"]&&!complete(s,m));
  let others=parent.and_then(|m|m["recipient_ids"].as_array()).is_some_and(|ids|ids.iter().any(|id|*id!=actor["id"]&&!messages.iter().any(|r|r["reply_to"]==args["reply_to"]&&r["agent_id"]==*id&&r["final"]==true)));
  if unfinished||others{return Err(err("Wait for assigned members to finish before finalizing the shared task"));}
 }Ok(())
}
pub(super) fn set_mode(s:&mut Value,value:&Value)->Result<()>{
 if *value!="group"&&*value!="work"{return Err(err("Invalid conversation mode"));}
 if s["mode"].as_str().unwrap_or("work")==value.as_str().unwrap(){return Ok(());}
 if s["closed"]==true{return Err(err("Conversation is closed"));}
 if *value=="group"{
  s["version"]=json!(2);s["mode"]=json!("group");s["members"]=json!([]);
  if s["attachment_id"].as_str().is_some_and(|v|!v.is_empty()){
   s["members"]=json!([{ "id":uuid::Uuid::new_v4().to_string(),"name":s["agent_name"].as_str().unwrap_or("AI"),"role":"coordinator","attachment_id":s["attachment_id"],"lease_until":s["lease_until"] }]);
  }if let Some(chief)=members(s).first(){for m in s["messages"].as_array_mut().unwrap(){if m["role"]=="assistant"&&m["agent_id"].is_null(){m["agent_id"]=chief["id"].clone();m["agent_name"]=chief["name"].clone();}}}s["attachment_id"]=json!("");s["lease_until"]=json!(0);bind_targets(s);
 }else{
  if pending_for(s,None,false).is_some()||s["queue"].as_array().is_some_and(|q|!q.is_empty()){return Err(err("Finish pending group tasks and outbox before switching to work"));}
  let all=members(s);let active:Vec<_>=all.iter().filter(|m|m["lease_until"].as_u64().unwrap_or(0)>now()).collect();
  if active.len()>1||active.iter().any(|m|m["role"]!="coordinator"){return Err(err("Disconnect assisting members before switching to work"));}
  let chief=all.iter().find(|m|m["role"]=="coordinator").cloned().unwrap_or(json!({"attachment_id":"","lease_until":0}));
  s["attachment_id"]=chief["attachment_id"].clone();s["lease_until"]=chief["lease_until"].clone();s["agent_name"]=chief["name"].clone();s["mode"]=json!("work");s["members"]=json!([]);
 }Ok(())
}
pub(super) fn ui(s:&mut Value,args:&Value)->Result<()>{
 if args["action"]=="set_mode"{return set_mode(s,&args["mode"]);}
 let member=members(s).into_iter().find(|m|m["id"]==args["member_id"]).ok_or_else(||err("Group member not found"))?;
 if args["action"]=="rename_member"{
  let name=member_name(&args["name"])?;if members(s).iter().any(|p|p["id"]!=member["id"]&&p["name"]==name){return Err(err("Agent name already exists"));}
  for p in s["members"].as_array_mut().unwrap(){if p["id"]==member["id"]{p["name"]=json!(name);}}
 }else if args["action"]=="detach_member"{for p in s["members"].as_array_mut().unwrap(){if p["id"]==member["id"]{p["lease_until"]=json!(0);p["paused"]=json!(true);}}}
 else if args["action"]=="resume_member"{for p in s["members"].as_array_mut().unwrap(){if p["id"]==member["id"]{p["paused"]=json!(false);}}}
 else if args["action"]=="set_coordinator"{
  if pending_for(s,None,false).is_some()||s["queue"].as_array().is_some_and(|q|!q.is_empty()){return Err(err("Finish pending tasks before changing coordinator"));}
  for p in s["members"].as_array_mut().unwrap(){p["role"]=json!(if p["id"]==member["id"]{"coordinator"}else{"member"});}
 }Ok(())
}
pub(super) fn markdown(s:&Value,m:Option<&Value>)->String{
 let all=members(s);let name=|id:&Value|all.iter().find(|p|p["id"]==*id).map(|p|p["name"].as_str().unwrap_or("")).unwrap_or_else(||id.as_str().unwrap_or("")).to_owned();
 if let Some(m)=m{
  let mut out=String::new();if let Some(n)=m["agent_name"].as_str(){out.push_str(&format!("\nAgent: {n} ({})\n",m["agent_id"].as_str().unwrap_or("")));}
  if let Some(ids)=m["recipient_ids"].as_array(){out.push_str(&format!("\nRecipients: {}\n",ids.iter().map(name).collect::<Vec<_>>().join(", ")));}
  if let Some(plans)=m["agent_plans"].as_array(){for p in plans{out.push_str(&format!("\n### {}\n{}",name(&p["agent_id"]),super::super::chat_plan::markdown(&p["plan"])));}}out
 }else if grouped(s){format!("Mode: group\n\n{}\n\n",all.iter().map(|p|format!("- {} · {} ({})",p["name"].as_str().unwrap_or(""),p["role"].as_str().unwrap_or(""),p["id"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n"))}else{String::new()}
}
