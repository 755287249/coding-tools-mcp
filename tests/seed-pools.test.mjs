import test from 'node:test';
import assert from 'node:assert/strict';
import {availablePools} from '../src/lib/chat/seed-pools.ts';
const groups=[
 {source:'coderabbit',prefix:'Astra',account:'one',ready:20},
 {source:'arena',prefix:'Astra',account:'two',ready:7},
 {source:'coderabbit',prefix:'Sol',account:'one',ready:15},
 {source:'custom',prefix:'Opus',account:'three',ready:3},
 {source:'coderabbit',prefix:'offline',account:'four',ready:0},
 {source:'custom',prefix:'',account:'two',ready:1},
];
test('pool categories merge ready counts and rank the two largest; zero-ready pools disappear',()=>{
 const prefix=availablePools(groups,'prefix');assert.deepEqual(prefix.slice(0,2).map(p=>[p.label,p.ready]),[['Astra',27],['Sol',15]]);
 assert.equal(prefix.length,4);assert.ok(!prefix.some(p=>p.label==='offline'));
 assert.deepEqual(availablePools(groups,'account').map(p=>[p.label,p.ready]),[['one',35],['two',8],['three',3]]);
 assert.deepEqual(availablePools(groups,'source').map(p=>[p.label,p.ready]),[['coderabbit',35],['arena',7],['custom',4]]);
 assert.deepEqual(availablePools([],'prefix'),[]);
});
test('modal source/search filters carry into exact pool selection including empty prefixes',()=>{
 assert.deepEqual(availablePools(groups,'prefix','AST','arena')[0].filter,{source:'arena',keyword:'',prefix:'Astra'});
 assert.equal(availablePools(groups,'prefix','AST','arena')[0].ready,7);
 assert.deepEqual(availablePools(groups,'account','two','arena')[0].filter,{source:'arena',keyword:'',account:'two'});
 assert.deepEqual(availablePools(groups,'prefix').find(p=>p.label==='').filter,{source:'',keyword:'',prefix:''});
 assert.deepEqual(availablePools(groups,'source','custom')[0].filter,{source:'custom',keyword:''});
 assert.deepEqual(availablePools(groups,'prefix','missing'),[]);
});
test('includeEmpty keeps zero-ready pools for a greyed chooser and standby sums only standby seeds',async()=>{
 const {standbyPool}=await import('../src/lib/chat/seed-pools.ts');
 const all=[{source:'coderabbit',prefix:'offline',account:'four',ready:0,total:3,standby:false},{source:'standby',prefix:'Claude',account:'Claude',ready:1,total:2,standby:true},{source:'coderabbit',prefix:'Sol',account:'one',ready:2,total:2,standby:false}];
 assert.deepEqual(availablePools(all,'prefix').map(p=>p.label),['Sol','Claude']);
 const shown=availablePools(all,'prefix','','',true);assert.deepEqual(shown.map(p=>[p.label,p.ready,p.total]),[['Sol',2,2],['Claude',1,2],['offline',0,3]]);
 assert.deepEqual(standbyPool(all),{ready:1,total:2,filter:{source:'',keyword:'',standby:true}});
});
