import test from 'node:test';
import assert from 'node:assert/strict';
import { restoreSharingSettings } from '../src/lib/sharing-settings.ts';
const profiles=[{id:'first',runtime:{local_port:28766}},{id:'second',runtime:{local_port:28767}}];
test('returning to sharing restores selected workspace, LAN and public URL from active origins',()=>{
  assert.deepEqual(restoreSharingSettings({enabled:true,origins:['http://127.0.0.1:28767','http://192.168.1.36:28767','https://share.example'],lanIp:'192.168.1.36'},profiles),{id:'second',lan:true,publicUrl:'https://share.example'});
});
test('public only and LAN only sharing retain their respective controls',()=>{
  assert.deepEqual(restoreSharingSettings({enabled:true,origins:['http://127.0.0.1:28766','https://share.example'],lanIp:'192.168.1.36'},profiles),{id:'first',lan:false,publicUrl:'https://share.example'});
  assert.deepEqual(restoreSharingSettings({enabled:true,origins:['http://127.0.0.1:28767','http://192.168.1.36:28767'],lanIp:'192.168.1.36'},profiles),{id:'second',lan:true,publicUrl:''});
});
test('disabled sharing and removed workspace do not select a fabricated profile',()=>{
  assert.deepEqual(restoreSharingSettings({enabled:false,origins:['https://old.example'],lanIp:null},profiles),{id:'first',lan:false,publicUrl:''});
  assert.deepEqual(restoreSharingSettings({enabled:true,origins:['invalid','http://127.0.0.1:28767'],lanIp:null},[]),{id:'',lan:false,publicUrl:''});
});

test('a changed temporary tunnel address replaces the saved public origin', async () => {
  const { refreshedOrigins, preferredSharingProfile } = await import('../src/lib/sharing-settings.ts');
  const local = 'http://127.0.0.1:3789', old = 'https://old-words.trycloudflare.com', now = 'https://new-words.trycloudflare.com';
  assert.deepEqual(refreshedOrigins([local, old], old, now), [local, now]);
  assert.equal(refreshedOrigins([local, old], old, old), null);
  assert.equal(refreshedOrigins([local, old], old, ''), null);
  assert.deepEqual(refreshedOrigins([local], '', now), [local, now]);
  assert.equal(preferredSharingProfile([{ id: 'a' }, { id: '00000000000000000000000000000001' }], 'a'), '00000000000000000000000000000001');
  assert.equal(preferredSharingProfile([{ id: 'a' }, { id: 'b' }], 'b'), 'b');
});
