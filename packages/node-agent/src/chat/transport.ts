import {createHash} from 'node:crypto';
import type {IncomingHttpHeaders} from 'node:http';
/** Same precedence as Desktop; only a fingerprint, never a raw bearer credential. */
export function activityClientIdentity(headers:IncomingHttpHeaders):string {
 const value=(name:string)=>typeof headers[name]==='string'?(headers[name] as string).trim():'';
 const explicit=value('x-openai-session');if(explicit)return explicit.slice(0,200);
 const session=value('mcp-session-id');if(session)return 'mcp-session:'+session.slice(0,200);
 const token=value('authorization');return (token?'client:':'anonymous:')+createHash('sha256').update(token||value('user-agent')||'unknown').digest('hex');
}
