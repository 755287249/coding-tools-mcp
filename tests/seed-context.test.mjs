import test from 'node:test';
import assert from 'node:assert/strict';
import {routingContext,observeRouting,pageSession} from '../plugins/coderabbit-seeds/context.mjs';
const route='/trpc/codingAgent.listTasks';
const h=(org,ws='')=>({'x-coderabbitai-organization':org,...(ws?{'x-coderabbitai-workspace':ws}:{}),authorization:'secret',cookie:'private'});
test('app session reads only expected storage keys and keeps token rotation separate from account identity',()=>{
 const values=new Map([['user',JSON.stringify({state:{user:{id:'account'},currentOrganization:{id:'org'}}})],['accessToken','session-a']]);
 const page={sessionStorage:{getItem:k=>{assert.ok(['user','accessToken'].includes(k));return values.get(k);}},localStorage:{getItem:k=>{assert.equal(k,'clerkGitProvider');return null;}}};
 const first=pageSession(page);assert.equal(first.account,'account');assert.equal(first.provider,'');assert.equal(first.access,'session-a');
 values.set('accessToken','session-b');assert.equal(pageSession(page).identity,first.identity);
 values.set('user',JSON.stringify({state:{user:{id:123}}}));assert.equal(pageSession(page),null);
});
test('routing observation only accepts same-origin tRPC and outputs two identifiers',()=>{
 assert.deepEqual(routingContext(route,h('123','workspace')), {organization:'123',workspace:'workspace'});
 assert.equal(routingContext('https://elsewhere.example/trpc/read',h('123')),null);assert.equal(routingContext('/assets',h('123')),null);
 assert.equal(routingContext(route,{}),null);assert.equal(routingContext(route,h('bad\nvalue')),null);
});
test('fetch observes Request and header overrides, clears omitted workspace, and preserves return/errors',async()=>{
 const calls=[],observed=[];const page={fetch(...args){calls.push(args);return Promise.resolve('unchanged');}};
 const original=observeRouting(page,c=>observed.push(c));
 assert.equal(await page.fetch(route,{headers:h('org','ws')}),'unchanged');
 await page.fetch(new Request('https://app.coderabbit.ai'+route,{headers:h('old','old-ws')}),{headers:h('new')});
 assert.deepEqual(observed,[{organization:'org',workspace:'ws'},{organization:'new',workspace:''}]);
 await original(route,{headers:h('manual')});assert.equal(observed.length,2);assert.equal(calls.length,3);
 const error=new Error('network');const bad={fetch(){throw error;}};observeRouting(bad,()=>{throw Error('observer');});
 assert.throws(()=>bad.fetch(route,{headers:h('org')}),e=>e===error);
});
test('XHR observes only selected headers, preserving request behavior',()=>{
 const observed=[];class XHR {open(...args){this.args=args;}setRequestHeader(){}send(){return 'sent';}}
 const page={fetch(){},XMLHttpRequest:XHR};observeRouting(page,c=>observed.push(c));
 const xhr=new XHR();xhr.open('GET',route);xhr.setRequestHeader('Authorization','secret');xhr.setRequestHeader('x-coderabbitai-organization','org');
 assert.equal(xhr.send(),'sent');assert.deepEqual(observed,[{organization:'org',workspace:''}]);
 xhr.open('GET','https://elsewhere.example/trpc/test');xhr.setRequestHeader('x-coderabbitai-organization','other');xhr.send();assert.equal(observed.length,1);
});
