import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {buildCompatPrompt} from '../src/lib/chat/compat-prompt.ts';
import {compatSkill} from '../packages/node-agent/src/chat/compat-skill.generated.ts';
test('GET prompt uses bounded independent grant and exact target, prefix and no OAuth',()=>{
 const p=buildCompatPrompt('https://example.test/prefix/mcp','a'.repeat(64),'chat-1','folder-1');
 assert.match(p,/https:\/\/example.test\/prefix\/mcp\/chat-compat/);assert.match(p,/"chat_id":"chat-1","workspace_folder_id":"folder-1"/);assert.match(p,/op=info/);assert.match(p,/op=open/);assert.match(p,/op=wait&timeout_ms=10000/);assert.match(p,/persisted=true/);assert.doesNotMatch(p,/oauth|refresh_token|bearer/i);
 for(const endpoint of ['file:///mcp','https://user:pass@host/mcp','https://host/mcp?key=x'])assert.throws(()=>buildCompatPrompt(endpoint,'a'.repeat(64),'c','f'));
});
test('packaged instructions equal source and lines fit file readers',()=>{
 assert.equal(compatSkill,readFileSync(new URL('../skills/chat-get-compat/SKILL.md',import.meta.url),'utf8').replace(/\r\n/g,'\n'));
 assert.ok(compatSkill.split('\n').every(l=>l.length<1000));
});
