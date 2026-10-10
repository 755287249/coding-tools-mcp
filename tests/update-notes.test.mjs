import test from 'node:test';
import assert from 'node:assert/strict';
import {displayReleaseNotes} from '../src/lib/update-notes.ts';
test('release display localizes packaging boilerplate while preserving meaningful changes',()=>{
 const notes='Windows standalone EXE. Download and run directly.\r\n\r\nCommit: '+'a'.repeat(40)+'\r\n\r\nSHA-256: `'+ 'b'.repeat(64)+'`\r\n\r\n修复手机消息配色\n\nSHA-256: improved integrity checks';
 assert.equal(displayReleaseNotes(notes,'Windows 免安装版，下载后即可使用。'),'Windows 免安装版，下载后即可使用。\n\n修复手机消息配色\n\nSHA-256: improved integrity checks');
 assert.equal(displayReleaseNotes('更新内容\n- 保留 **Markdown**\n- Fixed navigation','Portable'),'更新内容\n- 保留 **Markdown**\n- Fixed navigation');
 assert.equal(displayReleaseNotes('Commit: '+ 'a'.repeat(40),'Portable'),'');
});
