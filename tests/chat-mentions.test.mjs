import test from 'node:test';
import assert from 'node:assert/strict';
import { mentionParts,mentionQuery,mentionChoices,referencedAttachments } from '../src/lib/chat/mentions.ts';
import { innerPopover } from '../src/lib/chat/popover.ts';
import { uploadLocalFile,TRANSFER_CHUNK_BYTES,readChatFile } from '../src/lib/chat/attachment-transfer.ts';
import { createHash } from 'node:crypto';
const files=[{id:'a',label:'图片1',name:'image.png'},{id:'b',label:'图片12',name:'image.png'},{id:'f',label:'文件1',name:'report.md'}];
test('mentions resolve exact labels independently of filenames and numeric prefixes',()=>{
 const text='参考@图片12，然后@文件1，对比@图片1和@图片123';
 assert.deepEqual(referencedAttachments(text,files).map(f=>f.id),['b','f','a']);
 assert.equal(mentionParts('@图片123',files)[0].file,undefined);
 assert.deepEqual(mentionChoices(files,'图片').map(f=>f.id),['a','b']);
 assert.deepEqual(mentionChoices(files,'report').map(f=>f.id),['f']);
 assert.deepEqual(mentionQuery('中文@图片1 after',6),{start:2,end:6,query:'图片1'});
 assert.equal(mentionQuery('hello ',6),null);
});
test('popover goes right then up near the bottom, staying inside small viewports',()=>{
 assert.deepEqual(innerPopover({left:200,right:220,top:80,bottom:100},{width:300,height:140},{width:1000,height:600}),{left:228,top:80});
 assert.deepEqual(innerPopover({left:200,right:220,top:550,bottom:570},{width:300,height:140},{width:1000,height:600}),{left:228,top:430});
 const narrow=innerPopover({left:310,right:330,top:720,bottom:740},{width:340,height:200},{width:390,height:844});
 assert.ok(narrow.left>=8&&narrow.left+340<=382&&narrow.top+200<=836);
});
test('local upload over 2MiB uses bounded chunks and identical retry after a lost acknowledgment',async()=>{
 const file=new File([new Uint8Array(3*1024*1024+7)],'large.png');const seen=[],written=[];let dropped=false;
 const result=await uploadLocalFile(async args=>{seen.push(structuredClone(args));const bytes=Buffer.from(args.data_base64,'base64');assert.ok(bytes.length<=TRANSFER_CHUNK_BYTES);written[args.offset]=bytes;
 if(args.offset===0&&!dropped){dropped=true;throw Error('Lost acknowledgment')}
 return {next_offset:args.offset+bytes.length,...(args.offset+bytes.length===file.size?{attachment:{id:'large',size:file.size}}:{})};},'chat',file,'large');
 assert.deepEqual(seen[0],seen[1]);assert.equal(result.size,file.size);assert.equal(Buffer.concat(Object.values(written)).length,file.size);
});
test('range preview verifies full-file digest and rejects same-sized tampering',async()=>{
 const bytes=Buffer.from('test preview');const file={id:'f',size:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex')};
 const request=async()=>({attachment:file,next_offset:bytes.length,data_base64:bytes.toString('base64')});
 assert.equal(Buffer.from(await readChatFile(request,'chat',file)).toString(),'test preview');
 await assert.rejects(readChatFile(async()=>({...await request(),data_base64:Buffer.alloc(bytes.length).toString('base64')}),'chat',file),/changed/);
});
