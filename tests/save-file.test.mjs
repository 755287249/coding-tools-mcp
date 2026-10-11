import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {stripTypeScriptTypes} from 'node:module';
const source=readFileSync(new URL('../src/lib/save-file.ts',import.meta.url),'utf8').replace("'./browser-tools'",JSON.stringify(new URL('../src/lib/browser-tools.ts',import.meta.url).href));
const {saveFile}=await import('data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(source)).toString('base64'));
test('mobile save transfers every byte with bounded offsets and reports cancellation',async()=>{
 const calls=[],chunks=[];let fail=false;
 const bridge={onmessage:null,postMessage(raw){const data=JSON.parse(raw);calls.push(data);if(data.action==='chunk')chunks.push(Buffer.from(data.data,'base64'));queueMicrotask(()=>bridge.onmessage?.({data:JSON.stringify({requestId:data.requestId,ok:!fail,error:fail?'File save cancelled':undefined})}))}};
 globalThis.window={CodingToolsDownloads:bridge};
 const bytes=Buffer.alloc(300123);for(let i=0;i<bytes.length;i++)bytes[i]=i%251;
 await saveFile(new Blob([bytes],{type:'application/octet-stream'}),'large.bin');
 assert.deepEqual(Buffer.concat(chunks),bytes);assert.deepEqual(calls.filter(c=>c.action==='chunk').map(c=>c.offset),[0,131072,262144]);
 assert.equal(calls.at(-1).action,'finish');assert.equal(bridge.onmessage,null);
 fail=true;await assert.rejects(saveFile(new Blob(['x']),'cancel.txt'),/cancelled/);assert.equal(calls.at(-1).action,'cancel');
 fail=false;await saveFile(new Blob([]),'empty.txt');assert.equal(calls.at(-1).action,'finish');
 delete globalThis.window;
});
test('mobile save rejects overlapping dialogs without corrupting active transfer',async()=>{
 let message;const bridge={onmessage:null,postMessage(raw){message=JSON.parse(raw)}};globalThis.window={CodingToolsDownloads:bridge};
 const first=saveFile(new Blob([]),'a.txt');await assert.rejects(saveFile(new Blob([]),'b.txt'),/current file/);
 bridge.onmessage({data:JSON.stringify({requestId:message.requestId,ok:false,error:'cancelled'})});await assert.rejects(first,/cancelled/);delete globalThis.window;
});
