import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const url=source=>'data:text/javascript;base64,'+Buffer.from(ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ESNext,target:ts.ScriptTarget.ES2022}}).outputText).toString('base64');
const artifact=url(readFileSync(new URL('../src/lib/chat/artifact-links.ts',import.meta.url),'utf8'));
const source=readFileSync(new URL('../src/lib/chat/local-links.ts',import.meta.url),'utf8').replace("'./artifact-links'",JSON.stringify(artifact));
const {chatLinkParts,localPathTarget}=await import(url(source));
test('Windows absolute paths and workspace paths retain punctuation',()=>{
 const text='打开 C:\\Users\\belly\\Downloads\\1\\output\\push.cmd，或者 docs/chat-sessions/a.md。';
 const parts=chatLinkParts(text);assert.deepEqual(parts.filter(x=>x.path).map(x=>x.path),['C:\\Users\\belly\\Downloads\\1\\output\\push.cmd','docs/chat-sessions/a.md']);assert.equal(parts.map(x=>x.text).join(''),text);
});
test('labeled and code paths preserve spaces and unicode',()=>{
 assert.deepEqual(chatLinkParts('[报告](<C:\\My Files\\报告.md>)'),[{text:'报告',path:'C:\\My Files\\报告.md'}]);
 assert.deepEqual(chatLinkParts('output/my report.pdf',true),[{text:'output/my report.pdf',path:'output/my report.pdf'}]);
 assert.equal(chatLinkParts('README.md',true)[0].path,'README.md');
 assert.equal(localPathTarget('file:///C:/My%20Files/a.txt'),'C:/My Files/a.txt');
});
test('http links stay web links, and image preview recognition is preserved',()=>{
 assert.equal(chatLinkParts('[下载](https://example.com/a.zip)')[0].href,'https://example.com/a.zip');
 assert.equal(chatLinkParts('https://example.com/a.zip.')[0].href,'https://example.com/a.zip');
 assert.equal(chatLinkParts('mcp-assistant/artifacts/a.png')[0].image,true);
 assert.equal(chatLinkParts('mcp-assistant/artifacts/a.png.exe')[0].image,undefined);
});
test('schemes, UNC, parent traversal and alternate streams are never local actions',()=>{
 for(const path of ['javascript:alert(1)','C:relative','../outside.txt','docs/../secret.txt','C:\\file.txt:stream','\\\\server\\share','file://server/share','bad\npath'])assert.equal(localPathTarget(path),null,path);
 for(const text of ['[bad](javascript:alert(1))','\\\\server\\share','docs/../secret.txt'])assert.ok(chatLinkParts(text).every(p=>!p.path&&!p.href),text);
});
test('slash-separated prose, counts, versions and branch names stay plain text',()=>{
 for(const text of [
  '工作会话、群聊协调者/成员及恢复连接的下发一致性也已覆盖。',
  '桌面/Node 已同步；测试 29/29 通过，CI/CD 正常。',
  'coderabbit/connect-mcp-workspace/e5220af7',
  '/api/chat', 'src/lib', '0.1.87',
 ]) for(const literal of [false,true]) {
  const parts=chatLinkParts(text,literal);
  assert.ok(parts.every(part=>!part.path&&!part.href),text);
  assert.equal(parts.map(part=>part.text).join(''),text);
 }
});
test('ambiguous directories require explicit links while clear file paths keep working',()=>{
 assert.deepEqual(chatLinkParts('[源码目录](src/lib)'),[{text:'源码目录',path:'src/lib'}]);
 assert.equal(chatLinkParts('./src',true)[0].path,'./src');
 assert.equal(chatLinkParts('C:\\work\\demo',true)[0].path,'C:\\work\\demo');
 assert.equal(chatLinkParts('报告：docs/报告.md。').find(part=>part.path)?.path,'docs/报告.md');
 assert.equal(chatLinkParts('src/lib/main.ts',true)[0].path,'src/lib/main.ts');
});
