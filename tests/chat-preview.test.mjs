import test from 'node:test';
import assert from 'node:assert/strict';
import { buildPreviewDocument, previewLanguage, isTextFile } from '../src/lib/chat/preview.ts';

test('preview supports browser HTML/JS only and escapes script terminators', () => {
  assert.equal(previewLanguage('demo.HTML'),'html');
  assert.equal(previewLanguage('js'),'javascript');
  assert.equal(previewLanguage('component.tsx'),null);
  assert.equal(isTextFile('notes.md'),true);
  assert.equal(isTextFile('archive.zip'),false);
  const doc=buildPreviewDocument('console.log("</script><img src=x onerror=alert(1)>")','javascript');
  assert.equal((doc.match(/<\/script>/g)||[]).length,1);
  assert.match(doc,/\\u003c\/script>/);
  assert.throws(()=>buildPreviewDocument('汉'.repeat(90000),'html'),/256 KiB/);
});

test('HTML policy precedes supplied document and blocks privileged resource loads',()=>{
  const doc=buildPreviewDocument('<html><body><h1>Hello</h1></body></html>','html');
  assert.ok(doc.indexOf('Content-Security-Policy')<doc.indexOf('<h1>'));
  assert.match(doc,/connect-src 'none'/);
  assert.match(doc,/frame-src 'none'/);
  assert.match(doc,/form-action 'none'/);
  assert.match(doc,/base-uri 'none'/);
});
