import test from 'node:test';
import assert from 'node:assert/strict';
import {get} from 'svelte/store';
import {createUpdateDownload, downloadBusy, downloadPercent, formatDownloadBytes} from '../src/lib/update-download.ts';

const selection={repo:'owner/repo',version:'1.2.3',sha256:'a'.repeat(64)};
test('stream remains active across subscribers and blocks a duplicate transfer/reset until verification completes',async()=>{
 let report,complete,calls=0;
 const download=createUpdateDownload(async (received,progress)=>{
  assert.deepEqual(received,selection);calls++;report=progress;
  return new Promise(resolve=>{complete=resolve});
 });
 const pending=download.start(selection);
 assert.equal(get(download).phase,'connecting');
 report({phase:'downloading',downloaded:256,total:1024});
 assert.equal(downloadPercent(get(download)),25);
 const unsubscribe=download.subscribe(()=>{});unsubscribe();
 assert.equal(get(download).downloaded,256);
 assert.equal(download.reset(),false);
 await download.start(selection);assert.equal(calls,1);
 report({phase:'verifying',downloaded:1024,total:1024});
 assert.equal(downloadBusy(get(download)),true);
 complete({bytes:1024});await pending;
 assert.equal(get(download).phase,'ready');
 report({phase:'downloading',downloaded:1,total:null});
 assert.equal(get(download).phase,'ready','late IPC must not overwrite completion');
 assert.equal(download.reset(),true);assert.equal(get(download).phase,'idle');
});
test('unknown total is indeterminate; failed integrity check never enables install and retry resets bytes',async()=>{
 let attempt=0;
 const download=createUpdateDownload(async (_,report)=>{
  assert.equal(get(download).downloaded,0);
  report({phase:'downloading',downloaded:2048,total:null});
  assert.equal(downloadPercent(get(download)),undefined);
  if(attempt++===0)throw new Error('checksum mismatch');
  return {bytes:4096};
 });
 await download.start(selection);
 assert.equal(get(download).phase,'error');assert.match(get(download).error,/checksum/);
 assert.equal(downloadBusy(get(download)),false);
 await download.start(selection);
 assert.equal(get(download).phase,'ready');assert.equal(get(download).error,'');
 assert.equal(downloadPercent(get(download)),100);
 assert.equal(formatDownloadBytes(2*1024*1024),'2.0 MiB');
 assert.equal(formatDownloadBytes(2048),'2 KiB');
});
