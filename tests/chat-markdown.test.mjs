import test from 'node:test';
import assert from 'node:assert/strict';
import { chatMarkdownBlocks } from '../src/lib/chat/markdown.ts';

test('tables render header, alignment, Unicode and inline markup as structured text', () => {
  const [table] = chatMarkdownBlocks('| 项目 | 状态 | 耗时 |\n| :--- | :---: | ---: |\n| **测试** | `通过` | 2 秒 |');
  assert.deepEqual(table, { type: 'table', header: ['项目', '状态', '耗时'], align: ['left', 'center', 'right'], rows: [['**测试**', '`通过`', '2 秒']] });
});

test('outer pipes are optional and body columns are padded or truncated', () => {
  const [table] = chatMarkdownBlocks('Name | Value\n--- | ---\na | b | ignored\nc |');
  assert.deepEqual(table.rows, [['a', 'b'], ['c', '']]);
  assert.equal(chatMarkdownBlocks('| One |\n| --- |\n| only |')[0].header.length, 1);
  assert.deepEqual(chatMarkdownBlocks('| A | B |\n| --- | --- |\n| | |')[0].rows, [['', '']]);
});

test('escaped and code-span pipes remain cell content; unmatched ticks do not hide separators', () => {
  const [table] = chatMarkdownBlocks('| A | B |\n| --- | --- |\n| a\\|b | `x|y` |\n| ``a`|b`` | `x\\|y` |\n| unmatched ` | value |');
  assert.deepEqual(table.rows, [['a|b', '`x|y`'], ['``a`|b``', '`x|y`'], ['unmatched `', 'value']]);
});

test('ordinary pipe text, rules and malformed tables stay prose', () => {
  for (const text of ['a | b\nnot a delimiter', 'Heading\n---', '| a | b |\n| --- |', '| a | b |\n| --- | no |', 'a\\|b\n---']) {
    assert.ok(chatMarkdownBlocks(text).every(block => block.type === 'line'), text);
  }
});

test('tables end at prose and preserve mixed headings, lists and subsequent tables', () => {
  const blocks = chatMarkdownBlocks('# Title\n| A | B |\n| --- | --- |\n| 1 | 2 |\n- item\n\nX | Y\n- | -\n3 | 4');
  assert.deepEqual(blocks.map(block => block.type), ['line', 'table', 'line', 'line', 'table']);
  assert.equal(blocks[2].text, '- item');
});

test('fences preserve copyable code whitespace and prevent tables from parsing inside code', () => {
  const code = '  const x = 1;\n\tconsole.log(x);\n\n| A | B |\n| --- | --- |\n';
  const blocks = chatMarkdownBlocks('```javascript\n' + code + '```\nAfter');
  assert.deepEqual(blocks[0], { type: 'code', language: 'javascript', code });
  assert.equal(blocks[1].text, 'After');
  assert.deepEqual(chatMarkdownBlocks('````md\n```js\nhi\n```\n````')[0], { type: 'code', language: 'md', code: '```js\nhi\n```\n' });
  assert.deepEqual(chatMarkdownBlocks('~~~python\nprint(1)\n~~~')[0], { type: 'code', language: 'python', code: 'print(1)\n' });
});

test('unclosed streaming fences retain all code; CRLF is normalized without trimming', () => {
  assert.deepEqual(chatMarkdownBlocks('```js\r\n  x();\r\n')[0], { type: 'code', language: 'js', code: '  x();\n' });
  assert.equal(chatMarkdownBlocks('```text\nlast')[0].code, 'last');
  assert.equal(chatMarkdownBlocks('```text\n```')[0].code, '');
  const blocks = chatMarkdownBlocks('A | B\n--- | ---\n```language|label\ncode\n```');
  assert.deepEqual(blocks.map(block => block.type), ['table', 'code']);
});

test('HTML and link strings are data for the existing escaped renderer', () => {
  const [table] = chatMarkdownBlocks('| HTML | Link |\n| --- | --- |\n| <img src=x onerror=alert(1)> | [bad](javascript:alert(1)) |');
  assert.equal(table.rows[0][0], '<img src=x onerror=alert(1)>');
  assert.equal(table.rows[0][1], '[bad](javascript:alert(1))');
});
