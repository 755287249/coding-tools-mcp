import {randomId} from './browser-tools';
interface NativeDownloads {postMessage(message:string):void;onmessage:((event:{data:string})=>void)|null}
declare global {interface Window {CodingToolsDownloads?:NativeDownloads}}
let active=false;
/** Android uses an origin-restricted main-frame bridge and the system Save dialog. */
export async function saveFile(blob:Blob,name:string):Promise<void>{
 const bridge=window.CodingToolsDownloads;
 if(!bridge){const url=URL.createObjectURL(blob);const a=document.createElement('a');a.href=url;a.download=name;a.click();setTimeout(()=>URL.revokeObjectURL(url),60000);return;}
 if(active)throw Error('Finish the current file save first.');
 active=true;const previous=bridge.onmessage;
 const request=(data:Record<string,unknown>,timeout=30000)=>new Promise<Record<string,unknown>>((resolve,reject)=>{
  const requestId=randomId();const timer=setTimeout(()=>{bridge.onmessage=null;reject(Error('File save timed out.'));},timeout);
  bridge.onmessage=event=>{try{const reply=JSON.parse(event.data);if(reply.requestId!==requestId)return;clearTimeout(timer);bridge.onmessage=null;reply.ok!==true?reject(Error(reply.error||'Unable to save file.')):resolve(reply)}catch{/* Ignore unrelated native messages. */}};
  try{bridge.postMessage(JSON.stringify({...data,requestId}))}catch(error){clearTimeout(timer);bridge.onmessage=null;reject(error)}
 });
 try{
  await request({action:'start',name,mime:blob.type||'application/octet-stream',size:blob.size});
  for(let offset=0;offset<blob.size;offset+=128*1024){const bytes=new Uint8Array(await blob.slice(offset,offset+128*1024).arrayBuffer());let text='';for(let i=0;i<bytes.length;i+=8192)text+=String.fromCharCode(...bytes.subarray(i,i+8192));await request({action:'chunk',offset,data:btoa(text)});}
  await request({action:'finish'},10*60*1000);
 }catch(error){try{bridge.postMessage(JSON.stringify({action:'cancel',requestId:randomId()}))}catch{}throw error;}
 finally{bridge.onmessage=previous;active=false;}
}
