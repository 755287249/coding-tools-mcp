import test from 'node:test';
import assert from 'node:assert/strict';
import { prepareSharingRuntime } from '../src/lib/sharing-runtime.ts';

const profile = { id:'one', runtime:{bind_address:'127.0.0.1',local_port:28766} };
function fixture(state='running', confirm=true) {
  const calls=[];
  const actions={
    getStatus:async()=>({state,localMessage:'Please wait'}),
    confirmLan:async()=>{calls.push('confirm');return confirm},
    save:async p=>calls.push(['save',p.runtime.bind_address]),
    start:async()=>{calls.push('start');return {state:'running'}},
    restart:async()=>{calls.push('restart');return {state:'running'}},
  };
  return {calls,actions};
}
test('public sharing reuses running MCP without starting or restarting',async()=>{
  const f=fixture();assert.equal(await prepareSharingRuntime(profile,false,f.actions),profile);assert.deepEqual(f.calls,[]);
});
test('LAN confirms then saves and rebinds the running listener',async()=>{
  const f=fixture();const next=await prepareSharingRuntime(profile,true,f.actions);
  assert.equal(next.runtime.bind_address,'0.0.0.0');assert.equal(profile.runtime.bind_address,'127.0.0.1');
  assert.deepEqual(f.calls,['confirm',['save','0.0.0.0'],'restart']);
});
test('LAN repairs a previously saved wildcard address whose listener was not restarted',async()=>{
  const f=fixture();await prepareSharingRuntime({...profile,runtime:{...profile.runtime,bind_address:'0.0.0.0'}},true,f.actions);
  assert.deepEqual(f.calls,['confirm','restart']);
});
test('cancelling LAN leaves configuration and listener unchanged',async()=>{
  const f=fixture('running',false);assert.equal(await prepareSharingRuntime(profile,true,f.actions),null);assert.deepEqual(f.calls,['confirm']);
});
test('stopped listener starts after LAN configuration; transitional listener refuses changes',async()=>{
  const f=fixture('stopped');await prepareSharingRuntime(profile,true,f.actions);assert.deepEqual(f.calls,['confirm',['save','0.0.0.0'],'start']);
  for(const state of ['starting','stopping']){const f=fixture(state);await assert.rejects(prepareSharingRuntime(profile,true,f.actions),/Please wait/);assert.deepEqual(f.calls,[])}
});
test('startup error prevents enabling sharing',async()=>{
  const f=fixture('stopped');f.actions.start=async()=>({state:'error',localMessage:'Address unavailable'});
  await assert.rejects(prepareSharingRuntime(profile,false,f.actions),/Address unavailable/);
  const g=fixture();g.actions.restart=async()=>{throw Error('Restart failed')};
  await assert.rejects(prepareSharingRuntime(profile,true,g.actions),/Restart failed/);
});
