import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,mkdirSync,writeFileSync,rmSync,symlinkSync,realpathSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {resolveChatPath,fileManagerPlan} from '../dist/chat/reveal.js';
import {chatUi} from '../dist/chat/store.js';
test('clicked paths resolve without executing files or mutating a closed conversation',t=>{
 const root=mkdtempSync(path.join(tmpdir(),'chat-reveal-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
 mkdirSync(path.join(root,'with space'));writeFileSync(path.join(root,'with space','run.cmd'),'must not execute');
 const session=chatUi(root,{action:'create'}).session;chatUi(root,{action:'close',chat_id:session.id});
 const before=chatUi(root,{action:'read',chat_id:session.id}).session;
 const target=chatUi(root,{action:'reveal_path',chat_id:session.id,source_path:'with space/run.cmd'});
 assert.deepEqual(target,{path:realpathSync(path.join(root,'with space','run.cmd')),is_directory:false});
 assert.equal(resolveChatPath(root,'with space').is_directory,true);
 assert.deepEqual(chatUi(root,{action:'read',chat_id:session.id}).session,before);
 for(const value of ['../outside','C:relative','https://example.com','\\\\server\\share','file.txt:stream','bad\npath'])assert.throws(()=>resolveChatPath(root,value),undefined,value);
 assert.throws(()=>chatUi(root,{action:'reveal_path',chat_id:'missing',source_path:'with space'}));
});
test('relative symlink escape is rejected', {skip:process.platform==='win32'}, t=>{
 const root=mkdtempSync(path.join(tmpdir(),'chat-reveal-')),outside=mkdtempSync(path.join(tmpdir(),'chat-reveal-out-'));
 t.after(()=>{rmSync(root,{recursive:true,force:true});rmSync(outside,{recursive:true,force:true});});
 symlinkSync(outside,path.join(root,'escape'));assert.throws(()=>resolveChatPath(root,'escape'),/escapes/);
});
test('file manager plans select files and only open directories',()=>{
 const file='C:\\My Files\\run.cmd';assert.deepEqual(fileManagerPlan('win32',file,false),{command:'explorer.exe',args:['/select,',file]});
 assert.deepEqual(fileManagerPlan('win32',file,true).args,[file]);
 assert.deepEqual(fileManagerPlan('darwin','/tmp/script.sh',false),{command:'open',args:['-R','/tmp/script.sh']});
 assert.deepEqual(fileManagerPlan('linux','/tmp/script.sh',false),{command:'xdg-open',args:['/tmp']});
});
