import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';
const source=await readFile(new URL('../src/lib/quick-setup/startup.ts',import.meta.url),'utf8');
const js=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext}}).outputText;
const {startupSetupTarget}=await import('data:text/javascript;base64,'+Buffer.from(js).toString('base64'));

test('first launch guides setup and stopped workspaces are reused',()=>{
  assert.equal(startupSetupTarget('/',[],{}),'/quick-setup');
  assert.equal(startupSetupTarget('/',['one','two'],{one:'stopped',two:'error'},'two'),'/quick-setup?workspace=two');
  assert.equal(startupSetupTarget('/workspace/one',['one','two'],{one:'stopped',two:'running'}),'/quick-setup?workspace=one');
});
test('running, transitioning and unavailable status do not trigger setup',()=>{
  for(const state of ['running','starting','stopping',undefined]) {
    assert.equal(startupSetupTarget('/',['one'],{one:state}),null);
  }
  assert.equal(startupSetupTarget('/',['one','two'],{one:'stopped',two:'running'}),null);
});
test('startup preserves explicit setup, settings and missing workspace routes',()=>{
  for(const path of ['/quick-setup','/settings/general','/settings/updates','/workspace/missing']) {
    assert.equal(startupSetupTarget(path,['one'],{one:'stopped'}),null);
  }
});
