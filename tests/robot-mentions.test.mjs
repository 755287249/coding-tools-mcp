import test from 'node:test';
import assert from 'node:assert/strict';
import {robotMentionParts} from '../src/lib/chat/robot-mentions.ts';
test('robot mentions match routing aliases exactly and preserve text and member identity',()=>{
 const aliases={one:'CodeRabbit',two:'机器人-2',three:'bot_3'};
 const text='Hi #coderabbit，#机器人-2 / #bot_3! #CodeRabbitExtra #unknown #CodeRabbit';
 const parts=robotMentionParts(text,aliases);
 assert.equal(parts.map(p=>p.text).join(''),text);
 assert.deepEqual(parts.filter(p=>p.memberId).map(p=>[p.text,p.memberId]),[['#coderabbit','one'],['#机器人-2','two'],['#bot_3','three'],['#CodeRabbit','one']]);
 assert.deepEqual(robotMentionParts(text),[{text}]);
 assert.deepEqual(robotMentionParts('',aliases),[]);
});
