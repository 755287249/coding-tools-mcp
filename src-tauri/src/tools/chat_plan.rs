//! Durable chat plan reducer. Called while the chat storage lock is held.
use serde_json::{json,Value};
use super::workspace::WorkspaceError;
use crate::mcp::activity::{validate_todos,ActivityTodo};
fn invalid(message:impl Into<String>)->WorkspaceError{WorkspaceError::invalid_argument(message)}
fn field(args:&Value,key:&str,max:usize,required:bool)->Result<Option<String>,WorkspaceError>{
 match args.get(key){None|Some(Value::Null)=>if required{Err(invalid(format!("{key} is required")))}else{Ok(None)},Some(Value::String(value)) if value.chars().count()<=max&&(!required||!value.trim().is_empty())=>Ok(Some(super::redaction::redact_sensitive_text(value.trim()).0)),_=>Err(invalid(format!("Invalid {key}")))}
}
pub fn reduce(previous:&Value,name:&str,args:&Value,now:u64)->Result<Value,WorkspaceError>{
 if name=="report_progress" {
  let message=field(args,"message",2000,true)?.unwrap();let phase=field(args,"phase",160,false)?;let todo_id=field(args,"todo_id",80,false)?.filter(|s|!s.is_empty());
  let percent=match args.get("percent"){None|Some(Value::Null)=>None,Some(value)=>Some(value.as_u64().filter(|p|*p<=100).ok_or_else(||invalid("percent must be an integer from 0 to 100"))?)};
  let mut plan=if previous.is_object(){previous.clone()}else{json!({"goal":"","todos":[]})};let todos=plan["todos"].as_array().unwrap();
  if todo_id.as_ref().is_some_and(|id|!todos.iter().any(|t|t["id"]==*id)){return Err(invalid("Unknown todo_id"));}
  let todo_id=todo_id.or_else(||todos.iter().find(|t|t["status"]=="in_progress").and_then(|t|t["id"].as_str().map(str::to_string)));
  let mut progress=json!({"message":message,"updated_ms":now});if let Some(value)=phase{progress["phase"]=json!(value);}if let Some(value)=percent{progress["percent"]=json!(value);}if let Some(value)=todo_id{progress["todo_id"]=json!(value);}
  plan["progress"]=progress;plan["updated_ms"]=json!(now);return Ok(plan);
 }
 let key=if name=="set_todos"{"todos"}else{"plan"};let items=args[key].as_array().filter(|items|items.len()<=24).ok_or_else(||invalid(format!("{key} must contain at most 24 items")))?;
 let goal=field(args,"goal",400,false)?.unwrap_or_else(||previous["goal"].as_str().unwrap_or("").to_string());
 let explanation=if name=="update_plan"{field(args,"explanation",2000,false)?}else{None};
 let mut remaining=previous["todos"].as_array().cloned().unwrap_or_default();let mut used=std::collections::HashSet::new();let mut next_id=1;let mut todos=Vec::new();
 for item in items {
  let title=field(item,if name=="set_todos"{"title"}else{"step"},400,true)?.unwrap();
  let id=if name=="set_todos"{field(item,"id",80,true)?.unwrap()}else if let Some(index)=remaining.iter().position(|todo|todo["title"]==title){remaining.remove(index)["id"].as_str().unwrap().to_string()}else{loop{let candidate=format!("todo-{next_id}");next_id+=1;if !used.contains(&candidate)&&!remaining.iter().any(|t|t["id"]==candidate){break candidate;}}};
  used.insert(id.clone());let status=item["status"].as_str().unwrap_or("").to_string();todos.push(ActivityTodo{id,title,status});
 }
 validate_todos(&todos).map_err(invalid)?;if todos.is_empty(){return Ok(Value::Null);}
 let mut plan=json!({"goal":goal,"todos":todos,"updated_ms":now});if let Some(message)=explanation.filter(|s|!s.is_empty()){plan["progress"]=json!({"message":message,"updated_ms":now});}Ok(plan)
}
pub fn summary(plan:&Value)->Value{
 if plan.is_null(){return json!({"cleared":true,"todos":[]});}let mut result=plan.clone();let todos=plan["todos"].as_array().unwrap();let completed=todos.iter().filter(|t|t["status"]=="completed").count();let current=todos.iter().find(|t|t["status"]=="in_progress");
 result["completed"]=json!(completed);result["total"]=json!(todos.len());result["current"]=current.map(|t|json!({"id":t["id"],"title":t["title"]})).unwrap_or(Value::Null);result["status"]=json!(if !todos.is_empty()&&completed==todos.len(){"completed"}else if current.is_some(){"in_progress"}else{"pending"});result
}
pub fn markdown(plan:&Value)->String{
 if !plan.is_object(){return String::new();}let mut out=format!("\n### 任务计划\n\n{}\n\n",plan["goal"].as_str().unwrap_or(""));
 if let Some(todos)=plan["todos"].as_array(){for t in todos{out.push_str(&format!("- [{}] {} ({})\n",if t["status"]=="completed"{"x"}else{" "},t["title"].as_str().unwrap_or(""),t["status"].as_str().unwrap_or("")));}}
 if let Some(message)=plan["progress"]["message"].as_str(){out.push_str(&format!("\n{} {}",plan["progress"]["phase"].as_str().unwrap_or(""),message));if let Some(percent)=plan["progress"]["percent"].as_u64(){out.push_str(&format!(" · {percent}%"));}out.push('\n');}out
}
