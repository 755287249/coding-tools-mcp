import test from 'node:test';
import assert from 'node:assert/strict';
import {parseBundle,unwrapTrpc,createdTaskId,creationPayload,publicReceipt} from '../plugins/coderabbit-seeds/core.mjs';
const seed={seed_id:'00000000-0000-4000-a000-000000000001',ticket:'a'.repeat(64),repo_id:'exact-repo',branch:'feature/main',account:'test',expires_at:Date.now()+3600000};const bundle={version:1,endpoint:'https://example.com/client/a/mcp',workspace_folder_id:'project',seeds:[seed]};
test('plugin rejects expired, duplicate and untrusted batch targets',()=>{assert.deepEqual(parseBundle(JSON.stringify(bundle)),bundle);for(const bad of [{...bundle,endpoint:'http://example.com/mcp'},{...bundle,seeds:[seed,seed]},{...bundle,seeds:[{...seed,expires_at:0}]}])assert.throws(()=>parseBundle(JSON.stringify(bad)));});
test('plugin validates tRPC envelopes and real task IDs',()=>{assert.throws(()=>unwrapTrpc([{error:{message:'quota'}}]));assert.throws(()=>unwrapTrpc([{result:{}}]));assert.throws(()=>createdTaskId({ok:true}));assert.equal(createdTaskId(unwrapTrpc([{result:{data:{json:{taskId:seed.seed_id}}}}])),seed.seed_id);});
test('creation is repository exact and uses stable idempotency; receipts never contain credentials',()=>{const payload=creationPayload(bundle,seed);assert.equal(payload.repoId,'exact-repo');assert.equal(payload.branch_name,'feature/main');assert.equal(payload.clientRequestId,seed.seed_id);assert.ok(payload.source.instructions.includes('/client/a/mcp/seeds/project/'+seed.seed_id));const receipt=JSON.stringify(publicReceipt(seed,'account','created',seed.seed_id));assert.ok(!receipt.includes(seed.ticket));assert.ok(!receipt.includes('instructions'));});
test('new plugin tasks use only a capability URL and concise prompt; receipts exclude both credentials',()=>{
 const current={...seed,url_key:'b'.repeat(64)};
 assert.deepEqual(parseBundle(JSON.stringify({...bundle,seeds:[current]})).seeds[0],current);
 const prompt=creationPayload(bundle,current).source.instructions;
 assert.ok(prompt.startsWith(bundle.endpoint+'/seeds/project/'+current.url_key+' '));assert.ok(prompt.length<350);
 assert.ok(prompt.includes('chat_wait/chat_reply'));assert.ok(!prompt.includes(seed.ticket));assert.ok(!prompt.includes('initialize'));
 const receipt=JSON.stringify(publicReceipt(current,'account','created'));
 assert.ok(!receipt.includes(current.url_key));assert.ok(!receipt.includes(current.ticket));
 assert.throws(()=>parseBundle(JSON.stringify({...bundle,seeds:[{...current,url_key:'bad'}]})));
});
