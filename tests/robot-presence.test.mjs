import test from 'node:test';
import assert from 'node:assert/strict';
import {robotPresence} from '../src/lib/chat/robot-presence.ts';
test('robot status prioritizes offline over stale work/error and distinguishes active failures',()=>{
 assert.equal(robotPresence({status:'connected'}),'online');assert.equal(robotPresence({status:'connected',busy:true}),'working');assert.equal(robotPresence({status:'connected',busy:true,error:true}),'error');
 for(const status of ['offline','closed','expired'])assert.equal(robotPresence({status,busy:true,error:true}),'offline');
 assert.equal(robotPresence({status:'missing'}),'error');
});
