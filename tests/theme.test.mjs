import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const source=fs.readFileSync(new URL('../static/theme.js',import.meta.url),'utf8');
function boot({stored=null,dark=false,blocked=false}={}){
 const events=new Map(),mediaEvents=new Map();let value=stored;
 const root={dataset:{},style:{},classList:{toggle(name,on){this[name]=on;}}};
 const media={matches:dark,addEventListener(name,fn){mediaEvents.set(name,fn);}};
 const window={addEventListener(name,fn){events.set(name,fn);},dispatchEvent(event){events.get(event.type)?.(event);}};
 const localStorage={getItem(){if(blocked)throw Error('blocked');return value;},setItem(key,next){if(blocked)throw Error('blocked');value=next;}};
 vm.runInNewContext(source,{document:{documentElement:root},window,matchMedia:()=>media,localStorage,Event:class{constructor(type){this.type=type;}}});
 return {root,get saved(){return value;},choose(detail){window.dispatchEvent({type:'ctmcp-theme-preference',detail});},system(dark){media.matches=dark;mediaEvents.get('change')();},storage(newValue,key='theme'){window.dispatchEvent({type:'storage',key,newValue});}};
}
test('first paint defaults to system and follows live OS changes',()=>{
 const x=boot();assert.equal(x.root.dataset.themePreference,'system');assert.equal(x.root.dataset.theme,'light');
 x.system(true);assert.equal(x.root.dataset.theme,'dark');assert.equal(x.root.classList.dark,true);assert.equal(x.root.style.colorScheme,'dark');
});
test('explicit choice persists across reload and ignores OS changes until system selected',()=>{
 const x=boot({dark:true});x.choose('light');x.system(true);assert.equal(x.root.dataset.theme,'light');assert.equal(x.saved,'light');
 const reloaded=boot({stored:x.saved,dark:true});assert.equal(reloaded.root.dataset.theme,'light');
 x.choose('system');assert.equal(x.root.dataset.theme,'dark');assert.equal(x.saved,'system');
});
test('storage sync and unavailable/corrupt storage preserve a usable theme',()=>{
 const x=boot({stored:'invalid',dark:true});assert.equal(x.root.dataset.themePreference,'system');
 x.storage('light');assert.equal(x.root.dataset.theme,'light');x.storage(null,null);assert.equal(x.root.dataset.theme,'dark');
 const blocked=boot({blocked:true});blocked.choose('dark');assert.equal(blocked.root.dataset.theme,'dark');
});
