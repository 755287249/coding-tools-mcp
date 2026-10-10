import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {stripTypeScriptTypes} from 'node:module';
const source=readFileSync(new URL('../src/lib/chat/image-gallery.ts',import.meta.url),'utf8').replace("'./attachment-transfer'",JSON.stringify(new URL('../src/lib/chat/attachment-transfer.ts',import.meta.url).href));
const {attachmentImages,conversationImages,uniqueImages,imageKey}=await import('data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(source)).toString('base64'));
const file=(id,extra={})=>({id,path:`files/${id}.png`,name:`${id}.png`,mime:'image/png',size:100,sha256:'hash',...extra});
test('conversation gallery preserves message order, collapses repeated attachments and includes image artifacts',()=>{
 const first=file('first'),second=file('second');
 const result=conversationImages([
  {text:'[chart](mcp-assistant/artifacts/chart.png)',attachments:[first,file('text',{mime:'text/plain'})]},
  {text:'mcp-assistant/artifacts/chart.png。 mcp-assistant/artifacts/readme.md',attachments:[second,first]},
 ],'workspace','folder','chat');
 assert.deepEqual(result.map(x=>x.path),[first.path,'mcp-assistant/artifacts/chart.png',second.path]);
 assert.equal(result[0].file,first);assert.equal(result[1].file,undefined);
 assert.ok(result.every(x=>x.chatId==='chat'&&x.folderId==='folder'&&x.workspaceId==='workspace'));
});
test('draft gallery excludes non-images and oversized attachments and keeps draft order',()=>{
 const files=[file('second'),file('huge',{size:32*1024*1024+1}),file('doc',{mime:'text/plain'}),file('first',{label:'图片1'})];
 assert.deepEqual(attachmentImages(files,'w','f','c').map(x=>x.name),['second.png','图片1']);
 assert.deepEqual(attachmentImages([],'w','f','c'),[]);
});
test('same path in different chats or folders retains its own read scope',()=>{
 const image=attachmentImages([file('a')],'w','f','c')[0];
 const others=[{...image,chatId:'another'}, {...image,folderId:'another'}];
 assert.equal(uniqueImages([image,...others,image]).length,3);
 assert.notEqual(imageKey(image),imageKey(others[0]));
});
