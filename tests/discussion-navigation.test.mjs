import test from 'node:test';
import assert from 'node:assert/strict';
import {discussionLocation,discussionNavigation} from '../src/lib/chat/discussion-navigation.ts';
test('group routes preserve folder identity, never collide with work conversations',()=>{
 const url=new URL(discussionLocation('workspace / one','folder & two','same-id'),'http://localhost');
 assert.equal(url.searchParams.get('folder'),'folder & two');assert.equal(url.searchParams.get('discussion'),'same-id');assert.equal(url.searchParams.get('chat'),null);
 assert.equal(new URL(discussionLocation('w','f'),'http://localhost').searchParams.get('createGroup'),'1');
});
test('sidebar group presence derives only from current members and honors disconnect',()=>{
 const d={id:'a',name:'Team',members:['a'],updated_at:20,pinned:true};
 const sources=[{id:'a',status:'waiting',work_state:'processing'},{id:'b',status:'connected'}];
 const nav=discussionNavigation(d,sources);assert.equal(nav.id,'discussion:a');assert.equal(nav.discussionId,'a');assert.equal(nav.mode,'group');assert.equal(nav.pinned,true);assert.equal(nav.status,'connected');assert.equal(nav.work_state,'processing');
 assert.equal(discussionNavigation({...d,paused:true},sources).status,'offline');assert.equal(discussionNavigation({...d,paused:true},sources).work_state,null);
 assert.equal(discussionNavigation(d,[{...sources[0],closed:true},sources[1]]).status,'offline');
});
