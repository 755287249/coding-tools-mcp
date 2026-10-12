import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const moduleUrl=source=>'data:text/javascript;base64,'+Buffer.from(ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText).toString('base64');
const pairing=moduleUrl(readFileSync(new URL('../src/lib/chat/seed-pairing.ts',import.meta.url),'utf8'));
const {seedEnrollmentPrompt}=await import(moduleUrl(readFileSync(new URL('../src/lib/chat/seed-enrollment.ts',import.meta.url),'utf8').replace("'./seed-pairing'",JSON.stringify(pairing))));
const seed={seed_id:'seed-one',ticket:'a'.repeat(64),expires_at:Date.now()+3600000,source:'arena',prefix:'Opus'};
test('manual prompt enrolls only its seed and folder, exchanges credentials, and waits before project work',()=>{
 const prompt=seedEnrollmentPrompt('https://example.test/clients/mine/mcp','folder',seed);
 assert.ok(prompt.includes('https://example.test/clients/mine/mcp/seeds/folder/seed-one'));
 for(const term of ['seed_wait(timeout_ms:25000)','seed_access_token','chat_open','skill.text','Opus'])assert.ok(prompt.includes(term));
 assert.throws(()=>seedEnrollmentPrompt('https://example.test/mcp/seed-library/other','folder',seed));
 assert.throws(()=>seedEnrollmentPrompt('http://elsewhere.test/mcp','folder',seed));
 assert.throws(()=>seedEnrollmentPrompt('https://example.test/mcp','folder',{...seed,expires_at:0}));
});
test('URL-first manual prompt carries one capability URL, a ready web entry and the shared instruction',()=>{
 const url_key='b'.repeat(64),endpoint='https://example.test/clients/mine/mcp/seeds/folder/'+url_key,prompt=seedEnrollmentPrompt('https://example.test/clients/mine/mcp','folder',{...seed,url_key});
 assert.ok(prompt.includes('MCP 地址（Streamable HTTP，无需再认证）：'+endpoint+'\n'));
 // GET-only AIs must be handed a complete URL: many fetch tools refuse URLs the AI builds itself.
 const entry=prompt.split('\n').find(line=>line.startsWith('网页入口'));assert.ok(entry&&/：https:\/\/example\.test\/clients\/mine\/mcp\/seeds\/folder\/b{64}\/connect\?n=[a-z0-9]+$/.test(entry),entry);
 for(const word of ['chat_wait','chat_reply','seed_connect','Streamable HTTP','只能读网页','入库','不需要自己有 MCP 客户端'])assert.ok(prompt.includes(word),word);
 assert.ok(prompt.length<640,String(prompt.length));
 for(const secret of [seed.ticket,'Bearer','seed_access_token','seed_wait','无人值守','不要询问'])assert.ok(!prompt.includes(secret));
 assert.throws(()=>seedEnrollmentPrompt('https://example.test/mcp','folder',{...seed,url_key:'bad'}));
});
