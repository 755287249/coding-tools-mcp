import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {fileDrop} from '../src/lib/chat/file-drop.ts';
function fixture() {
 const doc=new EventTarget(),node=new EventTarget();node.ownerDocument=doc;node.contains=target=>target===node;
 let enabled=true;const active=[],received=[],errors=[];
 const options={enabled:()=>enabled,onFiles:files=>received.push(files),onActive:value=>active.push(value),onError:reason=>errors.push(reason)};
 const action=fileDrop(node,options);
 function fire(target,type,{files=[],types=['Files'],items=[]}={}) {const event=new Event(type,{cancelable:true});event.dataTransfer={types,files,items,dropEffect:'none'};target.dispatchEvent(event);return event;}
 return {doc,node,active,received,errors,options,action,fire,setEnabled:value=>enabled=value};
}
test('file and image drops are accepted as one batch, while text drops remain native',()=>{
 const f=fixture(),files=[new File(['hello'],'notes.txt'),new File(['image'],'photo.png',{type:'image/png'})];
 try{assert.equal(f.fire(f.node,'dragenter').defaultPrevented,true);assert.equal(f.active.at(-1),true);
 assert.equal(f.fire(f.node,'dragover').dataTransfer.dropEffect,'copy');
 assert.equal(f.fire(f.node,'drop',{files}).defaultPrevented,true);assert.deepEqual(f.received,[files]);assert.equal(f.active.at(-1),false);
 assert.equal(f.fire(f.node,'drop',{types:['text/plain']}).defaultPrevented,false);assert.equal(f.received.length,1);
 }finally{f.action.destroy()}
});
test('busy forms and folders cannot upload, and dropping outside preserves the page',()=>{
 const f=fixture(),files=[new File(['x'],'a.txt')];try{
 f.setEnabled(false);assert.equal(f.fire(f.node,'dragover').dataTransfer.dropEffect,'none');assert.equal(f.fire(f.node,'drop',{files}).defaultPrevented,true);assert.equal(f.received.length,0);
 f.setEnabled(true);f.fire(f.node,'drop',{files,items:[{webkitGetAsEntry:()=>({isDirectory:true})}]});assert.deepEqual(f.errors,['directory']);assert.equal(f.received.length,0);
 assert.equal(f.fire(f.doc,'dragover',{files}).defaultPrevented,true);assert.equal(f.fire(f.doc,'drop',{files}).defaultPrevented,true);assert.equal(f.received.length,0);
 }finally{f.action.destroy()}
 assert.equal(f.fire(f.doc,'drop',{files}).defaultPrevented,false);
});
test('nested drag events do not clear the highlight early and disabled updates reset it',async()=>{
 const f=fixture();try{
 f.fire(f.node,'dragenter');f.fire(f.node,'dragenter');f.fire(f.node,'dragleave');assert.equal(f.active.at(-1),true);f.fire(f.node,'dragleave');assert.equal(f.active.at(-1),false);
 f.fire(f.node,'dragenter');f.setEnabled(false);f.action.update(f.options);assert.equal(f.active.at(-1),false);
 f.setEnabled(true);f.action.update({...f.options,onFiles:async()=>{throw Error('fixture')}});f.fire(f.node,'drop',{files:[new File(['x'],'a.txt')]});await Promise.resolve();assert.deepEqual(f.errors,['upload']);
 }finally{f.action.destroy()}
});
test('main and additional Windows webviews permit HTML file drops',async()=>{
 const config=JSON.parse(await readFile(new URL('../src-tauri/tauri.conf.json',import.meta.url),'utf8'));
 assert.equal(config.app.windows.find(w=>w.label==='main').dragDropEnabled,false);
 const source=await readFile(new URL('../src/lib/backend/window.ts',import.meta.url),'utf8');
 let options;class WebviewWindow{constructor(label,value){options=value}async once(name,callback){if(name==='tauri://created')callback()}}
 const isolated=source.replace(/^import .*;$/mg,'').replaceAll('export ','');
 // Strip TypeScript only using the same runtime transform as production source tests.
 const {stripTypeScriptTypes}=await import('node:module');
 const run=new Function('desktopWindowApi','isDesktopWindow',stripTypeScriptTypes(isolated)+';return newAppWindow;')({WebviewWindow},()=>true);
 await run();assert.equal(options.dragDropEnabled,false);
});
