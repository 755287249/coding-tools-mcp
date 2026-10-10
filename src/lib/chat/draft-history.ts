import type { ChatFile } from '../api/chat';
export interface DraftSnapshot { text:string; attachments:ChatFile[]; start:number; end:number }
const clone=(value:DraftSnapshot):DraftSnapshot=>({...value,attachments:value.attachments.map(file=>({...file}))});
const identity=(value:DraftSnapshot)=>JSON.stringify([value.text,value.attachments]);
/** One conversation's unsent draft. File bytes stay in the attachment store. */
export function createDraftHistory(limit=100) {
  let entries:DraftSnapshot[]=[], position=-1, lastTyping=0;
  function reset(value:DraftSnapshot){entries=[clone(value)];position=0;lastTyping=0;}
  function record(value:DraftSnapshot,typing=false,now=Date.now()) {
    if(position<0){reset(value);return;}
    const previous=entries[position];
    if(identity(previous)===identity(value)){entries[position]=clone(value);return;}
    const sameFiles=JSON.stringify(previous.attachments)===JSON.stringify(value.attachments);
    const merge=typing&&sameFiles&&position>0&&position===entries.length-1&&lastTyping>0&&now-lastTyping<800;
    entries=entries.slice(0,position+1);
    if(merge)entries[position]=clone(value);
    else{entries.push(clone(value));if(entries.length>limit+1)entries.shift();position=entries.length-1;}
    lastTyping=typing&&sameFiles?now:0;
  }
  function move(delta:number){lastTyping=0;const next=position+delta;if(next<0||next>=entries.length)return null;position=next;return clone(entries[position]);}
  return {reset,record,undo:()=>move(-1),redo:()=>move(1)};
}
