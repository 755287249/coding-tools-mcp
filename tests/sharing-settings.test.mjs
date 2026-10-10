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
