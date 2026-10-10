import test from 'node:test';
import assert from 'node:assert/strict';
import {mobileViewport} from '../src/lib/chat/mobile-viewport.ts';

test('keyboard bursts use the latest viewport once per frame, skip unchanged writes and clean up', t => {
  const viewport = Object.assign(new EventTarget(), {height:700, offsetTop:0, scale:1});
  const media = Object.assign(new EventTarget(), {matches:true});
  const frames = new Map(); let next = 0, writes = 0;
  const styles = new Map();
  const node = {style:{setProperty:(key,value)=>{writes++;styles.set(key,value);},removeProperty:key=>styles.delete(key)}};
  const old = {window:globalThis.window,requestAnimationFrame:globalThis.requestAnimationFrame,cancelAnimationFrame:globalThis.cancelAnimationFrame};
  Object.assign(globalThis, {
    window:{visualViewport:viewport,matchMedia:()=>media},
    requestAnimationFrame:callback=>{frames.set(++next,callback);return next;},
    cancelAnimationFrame:id=>frames.delete(id),
  });
  let action;
  t.after(()=>{action?.destroy();Object.assign(globalThis,old);});
  const flush=()=>{const callbacks=[...frames.values()];frames.clear();callbacks.forEach(callback=>callback());};
  action=mobileViewport(node);
  assert.equal(styles.get('--mobile-viewport-height'),'700px');
  for(let i=0;i<20;i++){
    viewport.height=500-i;viewport.offsetTop=i;
    viewport.dispatchEvent(new Event('resize'));viewport.dispatchEvent(new Event('scroll'));
  }
  assert.equal(frames.size,1);
  assert.equal(writes,2);
  flush();
  assert.equal(styles.get('--mobile-viewport-height'),'481px');
  assert.equal(styles.get('--mobile-viewport-top'),'19px');
  assert.equal(writes,4);
  viewport.dispatchEvent(new Event('resize'));flush();assert.equal(writes,4);
  viewport.scale=2;viewport.dispatchEvent(new Event('resize'));flush();assert.equal(styles.size,0);
  viewport.scale=1;viewport.dispatchEvent(new Event('resize'));flush();assert.equal(styles.size,2);
  media.matches=false;media.dispatchEvent(new Event('change'));flush();assert.equal(styles.size,0);
  media.matches=true;media.dispatchEvent(new Event('change'));
  action.destroy();assert.equal(frames.size,0);assert.equal(styles.size,0);
  viewport.dispatchEvent(new Event('resize'));media.dispatchEvent(new Event('change'));
  assert.equal(frames.size,0);
});
