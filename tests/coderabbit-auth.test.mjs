import test from 'node:test';
import assert from 'node:assert/strict';
import { mailReceivedAt, quota } from '../plugins/coderabbit/src/auth.ts';

test('mail timestamps without a zone are interpreted as UTC+8', () => {
  assert.equal(mailReceivedAt('2026-10-13 02:00:00'), Date.parse('2026-10-13T02:00:00+08:00'));
  assert.equal(mailReceivedAt('2026-10-12T18:00:00Z'), Date.parse('2026-10-12T18:00:00Z'));
  assert.equal(Number.isNaN(mailReceivedAt('')), true);
});

test('quota combines current and monthly seconds and sends cookie plus bearer', async () => {
  let request;
  const http = async value => { request=value;return {status:200,headers:{},body:JSON.stringify([{result:{data:{json:{agent:{remainingSeconds:1200,monthlyRemainingSeconds:4200}}}}}])}; };
  const session={access:'token',account:'a',accountName:'a',providerUserId:'',provider:'github',organization:'o',workspace:'',orgName:'o',providerOrg:'o',orgType:'github',expiresAt:0};
  const value=await quota({email:'a',mailApiUrl:'',password:'',session,cookies:[{name:'cr-session',value:'cookie',domain:'app.coderabbit.ai',path:'/',secure:true,hostOnly:true}]},http);
  assert.equal(value,90);
  assert.equal(request.headers.authorization,'Bearer token');
  assert.equal(request.headers.cookie,'cr-session=cookie');
});
