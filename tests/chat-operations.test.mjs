import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {stripTypeScriptTypes} from 'node:module';
const source=readFileSync(new URL('../src/lib/chat/operations.ts',import.meta.url),'utf8');
const {chatOperations,operationSummary}=await import('data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(source)).toString('base64'));
test('operation scope includes assignments, distinguishes evidence, and excludes other requests',()=>{
 const event={id:'op1',reply_to:'assignment',tool:'read_file',paths:['a.ts'],agent_name:'Builder',status:'completed',started_at:4};
 const session={agent_name:'Chief',operations:[event,{...event,id:'op2',reply_to:'old',started_at:1}],messages:[{id:'u',role:'user',text:'Build'},{id:'assignment',kind:'assignment',reply_to:'u',text:'Inspect'},{id:'report',role:'assistant',reply_to:'assignment',agent_name:'Builder',tool_event:{name:'cloud tool',status:'completed'},created_at:3},{id:'old',role:'user',text:'Previous'}]};
 const current=chatOperations(session,'u');assert.equal(current.length,2);assert.equal(current[0].source,'mcp');assert.equal(current[0].task,'Inspect');assert.equal(current[1].source,'reported');assert.equal(current[1].agent_name,'Builder');assert.equal(chatOperations(session).length,3);assert.deepEqual(chatOperations(null),[]);
});

test('success metrics exclude unknown/running and AI reports; edits exclude previews and failed writes',()=>{
 const row={source:'mcp',kind:'read',status:'completed',paths:['one.ts'],duration_ms:10};
 const rows=[row,{...row,kind:'edit',paths:['one.ts','two.ts'],duration_ms:30},{...row,kind:'edit',paths:['two.ts'],duration_ms:20},{...row,status:'failed',kind:'exec',duration_ms:100},{...row,status:'running',duration_ms:200},{...row,status:'interrupted',duration_ms:500},{...row,source:'reported',status:'completed'},{...row,kind:'edit',dry_run:true,paths:['preview.ts']},{...row,kind:'edit',status:'failed',paths:['failed.ts']}];
 const stats=operationSummary(rows);assert.equal(stats.total,8);assert.equal(stats.completed,4);assert.equal(stats.failed,2);assert.equal(stats.successRate,66.7);assert.equal(stats.running,1);assert.equal(stats.unknown,1);assert.equal(stats.reported,1);assert.equal(stats.files,2);assert.equal(stats.p95,100);assert.equal(stats.average,30);
 assert.equal(operationSummary([]).successRate,null);assert.equal(operationSummary([{...row,status:'running'}]).p95,null);
});
