import test from 'node:test';
import assert from 'node:assert/strict';
import {createReadingPositionCache,chatReadingPositions} from '../src/lib/chat/view-cache.ts';
import {clearChatCaches} from '../src/lib/chat/cache-lifecycle.js';
const position={historyAnchor:'old-message',scrollTop:1480,following:false,activeMessage:'old-message'};
test('reading range and position survive reopening with isolated bounded entries',()=>{
 const cache=createReadingPositionCache(2);cache.put('a',position);cache.put('b',{...position,scrollTop:5});
 const a=cache.get('a');assert.deepEqual(a,position);a.scrollTop=0;assert.equal(cache.get('a').scrollTop,1480);
 cache.put('c',position);assert.equal(cache.get('b'),null);assert.ok(cache.get('a'));
});
test('logout clears positions and a late unmount cannot restore the previous identity',()=>{
 const version=chatReadingPositions.version();chatReadingPositions.put('a',position,version);clearChatCaches();
 chatReadingPositions.put('a',position,version);assert.equal(chatReadingPositions.get('a'),null);
 chatReadingPositions.put('a',position);assert.deepEqual(chatReadingPositions.get('a'),position);clearChatCaches();
});
