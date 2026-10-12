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
 // Short and unambiguous: one URL, and the server explains rules and tools itself.
 assert.ok(prompt.includes(' '+endpoint+' 。'),prompt);
 for(const word of ['请连接我的工具','明确使用规则','熟悉可用工具','工具在 MCP 内部，而非你的客户端内部','然后等待信息'])assert.ok(prompt.includes(word),word);
 assert.ok(prompt.replace(endpoint,'').length<120,String(prompt.length));
 for(const secret of [seed.ticket,'Bearer','seed_access_token','seed_wait','无人值守','不要询问'])assert.ok(!prompt.includes(secret));
 assert.throws(()=>seedEnrollmentPrompt('https://example.test/mcp','folder',{...seed,url_key:'bad'}));
});
