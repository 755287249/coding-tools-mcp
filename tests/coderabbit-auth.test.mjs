import test from 'node:test';
import assert from 'node:assert/strict';
import { mailReceivedAt, quota, sortAccounts, cookieHttp } from '../plugins/coderabbit/src/auth.ts';

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

test('quota is a GET tRPC query with the organization header (POST answers 405 live)', async () => {
  let seen;
  const http = async req => { seen = req; return { status: 200, headers: {}, body: JSON.stringify([{ result: { data: { agent: { remainingSeconds: 6000 } } } }]) }; };
  const minutes = await quota({ email: 'a', mailApiUrl: 'b', password: 'c', session: { access: 't', organization: 'org', provider: 'github', workspace: 'w' } }, http);
  assert.equal(minutes, 100); assert.equal(seen.method, 'GET'); assert.match(seen.url, /onDemandProducts\.access\?batch=1&input=/);
  assert.equal(seen.headers['x-coderabbitai-organization'], 'org'); assert.equal(seen.body, undefined);
});

test('accounts sort by smallest positive quota; zero and unknown go last', () => {
  const sorted = sortAccounts([{ e: 'z', quota: 0 }, { e: 'b', quota: 100 }, { e: 'u' }, { e: 'a', quota: 20 }]);
  assert.deepEqual(sorted.map(a => a.e), ['a', 'b', 'z', 'u']);
});

test('cookieHttp sends stored cookies and saves new ones on the account', async () => {
  const account = { email: 'a', mailApiUrl: 'b', password: 'c' };
  const calls = [];
  const http = async req => { calls.push(req); return { status: 200, headers: { 'set-cookie': ['cr-session=abc; Path=/; Secure'] }, body: '[]' }; };
  const wrapped = cookieHttp(account, http);
  await wrapped({ url: 'https://app.coderabbit.ai/trpc/x?batch=1', method: 'GET', headers: {} });
  await wrapped({ url: 'https://app.coderabbit.ai/trpc/y?batch=1', method: 'GET', headers: {} });
  assert.equal(calls[0].headers.cookie, undefined); assert.match(calls[1].headers.cookie, /cr-session=abc/); assert.ok(account.cookies);
});
