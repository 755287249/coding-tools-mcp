import test from 'node:test';
import assert from 'node:assert/strict';
import {boundPreview,centerPreview,resizePreview,PREVIEW_HEADER_HEIGHT} from '../src/lib/chat/preview-window.ts';
test('preview centers on desktop and remains reachable on a narrow viewport',()=>{
 const desktop=centerPreview({width:1440,height:1000});
 assert.equal(desktop.x,(1440-desktop.width)/2);assert.equal(desktop.y,(1000-desktop.height)/2);
 const narrow=boundPreview({...desktop,x:1400,y:900},{width:320,height:480});
 assert.ok(narrow.x>=8&&narrow.y>=8);assert.ok(narrow.x+narrow.width<=312&&narrow.y+narrow.height<=472);
});
test('move and resize clamp extremes without drifting the resize origin',()=>{
 const rect={x:140,y:80,width:500,height:350},viewport={width:1000,height:800};
 const large=resizePreview(rect,10000,10000,viewport);
 assert.equal(large.x,rect.x);assert.equal(large.y,rect.y);assert.equal(large.width,852);assert.equal(large.height,712);
 const small=resizePreview(rect,-10000,-10000,viewport);assert.equal(small.width,280);assert.equal(small.height,220);
 assert.deepEqual(boundPreview({...rect,x:-999,y:-999},viewport),{...rect,x:8,y:8});
});
test('minimized title stays visible at the bottom and restoring reclaims full bounds',()=>{
 const viewport={width:900,height:600};
 const minimized=boundPreview({x:40,y:999,width:500,height:400},viewport,true);
 assert.equal(minimized.y+PREVIEW_HEADER_HEIGHT,592);
 const restored=boundPreview(minimized,viewport);
 assert.equal(restored.height,400);assert.equal(restored.y+restored.height,592);
 const tiny=centerPreview({width:200,height:150});assert.ok(tiny.width<=184&&tiny.height<=134);
});
