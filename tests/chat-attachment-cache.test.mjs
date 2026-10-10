import test from 'node:test';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createAttachmentCache,attachmentCache} from '../src/lib/chat/attachment-cache.js';
import {readChatFile} from '../src/lib/chat/attachment-transfer.ts';
import {clearChatCaches} from '../src/lib/chat/cache-lifecycle.js';

test('verified attachments are reused by scope and digest; corrupt data never enters cache',async()=>{
 clearChatCaches();const bytes=Buffer.from('synthetic image bytes');
 const file={id:'f',name:'image.png',mime:'image/png',size:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex')};
 let calls=0;const load=async()=>{calls++;return {data_base64:bytes.toString('base64'),attachment:file,next_offset:bytes.length}};
 const [a,b]=await Promise.all([readChatFile(load,'chat',file,['w','f']),readChatFile(load,'chat',file,['w','f'])]);assert.equal(calls,1);assert.deepEqual(a,b);
 a.fill(0);assert.deepEqual(await readChatFile(load,'chat',file,['w','f']),b);assert.equal(calls,1);
 await readChatFile(load,'chat',file,['w','other']);assert.equal(calls,2);
 const invalid={...file,sha256:'0'.repeat(64)};let bad=0;
 for(let i=0;i<2;i++)await assert.rejects(readChatFile(async()=>{bad++;return {...await load(),attachment:invalid}},'chat',invalid,['w','f']),/changed/);
 assert.equal(bad,2);clearChatCaches();await readChatFile(load,'chat',file,['w','f']);assert.equal(calls,5);
});

test('byte and entry bounds evict least recently used; reset fences pending reads',async()=>{
 const cache=createAttachmentCache(4,2);let loads=0;
 const read=(key)=>cache.read(key,async()=>{loads++;return new Uint8Array([1,2])});
 await read('a');await read('b');await read('a');await read('c');await read('b');assert.equal(loads,4);
 let resolve;const pending=cache.read('late',()=>new Promise(r=>resolve=r)),rejected=assert.rejects(pending,/identity/);
 await Promise.resolve();cache.clear();resolve(new Uint8Array([3]));await rejected;
 await read('late');assert.equal(loads,5);
});
