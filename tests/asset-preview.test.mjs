import test from 'node:test';
import assert from 'node:assert/strict';
import { assetPreview, MAX_ASSET_IMAGE_BYTES, MAX_ASSET_TEXT_BYTES } from '../src/lib/chat/asset-preview.ts';
const file=(name,size=100,mime='')=>({name,size,mime});
test('gallery identifies images and bounded plain text without rendering HTML',()=>{
  assert.deepEqual(assetPreview(file('photo.PNG'),true),{kind:'image',mime:'image/png'});
  assert.deepEqual(assetPreview(file('note.bin',100,'image/webp'),true),{kind:'image',mime:'image/webp'});
  for(const name of ['note.md','script.ts','markup.html','vector.svg'])assert.deepEqual(assetPreview(file(name),true),{kind:'text'});
});
test('large or binary attachments use a file tile without automatic downloads',()=>{
  assert.equal(assetPreview(file('photo.png',MAX_ASSET_IMAGE_BYTES+1),true),null);
  assert.equal(assetPreview(file('note.txt',MAX_ASSET_TEXT_BYTES+1),true),null);
  for(const name of ['portable.exe','archive.zip','book.pdf'])assert.equal(assetPreview(file(name),true),null);
});
test('path-only text/binary references never request attachment reads; image references use artifact preview',()=>{
  assert.deepEqual(assetPreview(file('plot.png',0),false),{kind:'image',mime:'image/png'});
  assert.equal(assetPreview(file('report.md',0),false),null);
  assert.equal(assetPreview(file('file.zip',0),false),null);
});
