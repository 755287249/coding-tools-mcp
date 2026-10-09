import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,writeFile,readFile,rm,chmod} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawnSync} from 'node:child_process';
const wrapper=fileURLToPath(new URL('../scripts/verify-frontend.mjs',import.meta.url));
for (const native of [false,true]) {
 test(`frontend verification invokes ${native?'native':'JavaScript'} CLI and propagates failure`, {skip:native&&process.platform==='win32'}, async t => {
  const root=await mkdtemp(path.join(tmpdir(),'frontend-launch-'));t.after(()=>rm(root,{recursive:true,force:true}));
  const cli=path.join(root,native?'package-manager':'package-manager.cjs');
  const log=path.join(root,'calls.log');
  await writeFile(cli,native?'#!/bin/sh\nprintf "%s\\n" "$*" >> "$CALL_LOG"\n[ "$2" != "test" ]\n':"require('node:fs').appendFileSync(process.env.CALL_LOG,process.argv.slice(2).join(' ')+'\\n');process.exit(process.argv[3]==='test'?1:0);\n");
  if(native)await chmod(cli,0o755);
  const result=spawnSync(process.execPath,[wrapper],{cwd:root,env:{...process.env,npm_execpath:cli,CALL_LOG:log},encoding:'utf8'});
  assert.equal(result.status,1,result.stderr);
  assert.deepEqual((await readFile(log,'utf8')).trim().split('\n').sort(),['run check','run test']);
 });
}
