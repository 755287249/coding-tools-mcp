// Project share link: one URL carries both the credential and the project.
// Mirrors src-tauri/src/tools/chat_share.rs; the file format is shared.
import {createHash,randomBytes,timingSafeEqual} from 'node:crypto';
import {existsSync,mkdirSync,readFileSync,renameSync,rmSync,statSync,writeFileSync} from 'node:fs';
import path from 'node:path';

export type ShareAccess='readonly'|'full';
export interface ShareStore {file:string;legacy:string}
/** v0.1.158 stored the link as `share-link.json`, which the chat list read as a chat. */
export const LEGACY_SHARE_STEM='share-link';
interface ShareRecord {version:1;key_hash:string;access:ShareAccess;created_at:number}

const digest=(key:string)=>createHash('sha256').update(key).digest('hex');
export const validShareKey=(key:string)=>/^[a-f0-9]{64}$/.test(key);

/** Moves the v0.1.158 file out of the chat ID namespace. Best effort and idempotent. */
export function migrateLegacyShare(store:ShareStore):void {
  try{
    const value=JSON.parse(readFileSync(store.legacy,'utf8'));
    if(value?.version!==1||typeof value.key_hash!=='string'||'messages' in value)return;
    if(existsSync(store.file))rmSync(store.legacy,{force:true});else renameSync(store.legacy,store.file);
  }catch{/* missing or not a share file */}
}
function read(store:ShareStore):ShareRecord|undefined {
  migrateLegacyShare(store);
  let size:number;
  try{size=statSync(store.file).size;}catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')return undefined;throw error;}
  if(size>4096)throw Error('Invalid share link file');
  const value=JSON.parse(readFileSync(store.file,'utf8')) as ShareRecord;
  return value?.version===1?value:undefined;
}
const status=(record?:ShareRecord)=>record?{ok:true,enabled:true,access:record.access,created_at:record.created_at}:{ok:true,enabled:false};

/** UI actions; the caller holds the chat storage lock for mutations. */
export function shareUi(store:ShareStore,args:Record<string,unknown>):Record<string,unknown> {
  switch(args.action){
    case 'share_status':return status(read(store));
    case 'share_revoke':rmSync(store.file,{force:true});return status();
    case 'share_create':{
      const access=args.access??'readonly';
      if(access!=='readonly'&&access!=='full')throw Error('access must be readonly or full');
      const key=randomBytes(32).toString('hex');
      const record:ShareRecord={version:1,key_hash:digest(key),access,created_at:Date.now()};
      mkdirSync(path.dirname(store.file),{recursive:true});
      const tmp=store.file+'.tmp';writeFileSync(tmp,JSON.stringify(record));renameSync(tmp,store.file);
      return {...status(record),url_key:key};
    }
    default:throw Error('Unknown share action');
  }
}

/** Returns the access mode for a valid key. Old links stop working after regenerate or revoke. */
export function shareAuthenticate(store:ShareStore,key:string):ShareAccess {
  if(!validShareKey(key))throw Error('Share link invalid or revoked');
  const record=read(store);
  const expected=Buffer.from(record?.key_hash??''),actual=Buffer.from(digest(key));
  if(!record||expected.length!==actual.length||!timingSafeEqual(expected,actual))throw Error('Share link invalid or revoked');
  return record.access==='full'?'full':'readonly';
}
