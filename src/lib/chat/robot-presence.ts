/** Offline always wins over stale work/error evidence. Missing source data is abnormal. */
export function robotPresence(member:{status:string;busy?:boolean;error?:boolean}):'online'|'offline'|'error'|'working' {
 if(member.status==='missing'||member.status==='error')return 'error';
 if(member.status!=='connected'&&member.status!=='waiting')return 'offline';
 if(member.error)return 'error';
 return member.busy?'working':'online';
}
