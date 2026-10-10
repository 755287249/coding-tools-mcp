import test from 'node:test';
import assert from 'node:assert/strict';
import { chatMarkdownBlocks, markdownCode, markdownHref, markdownText } from '../src/lib/chat/markdown.ts';

const content = source => chatMarkdownBlocks(source).filter(t=>t.type!=='space');
test('CommonMark headings 1–6, setext, nested lists, quotes and GFM tasks',()=>{
 for(let n=1;n<=6;n++)assert.equal(content('#'.repeat(n)+' Title')[0].depth,n);
 assert.equal(content('Title\n===')[0].depth,1);
 const [quote,list]=content('> **quoted**\n>\n> second paragraph\n\n3. first\n   - [x] nested\n   - [ ] pending\n4. second');
 assert.equal(quote.type,'blockquote');assert.equal(quote.tokens.filter(t=>t.type==='paragraph').length,2);
 assert.equal(list.start,3);assert.equal(list.items.length,2);
 const nested=list.items[0].tokens.find(t=>t.type==='list');assert.equal(nested.items[0].checked,true);assert.equal(nested.items[1].checked,false);
});
test('GFM tables preserve escaped pipes, inline code, alignment and missing cells',()=>{
 const [table]=content('| 项目 | 状态 | 数值 |\n| :--- | :---: | ---: |\n| **测试** | `x\\|y` | 2 |\n| a\\|b | c |');
 assert.equal(table.type,'table');assert.deepEqual(table.align,['left','center','right']);
 assert.equal(table.rows[0][1].tokens[0].text,'x|y');assert.equal(table.rows[1][0].text,'a|b');assert.equal(table.rows[1][2].text,'');
 assert.notEqual(content('| a | b |\n| --- |')[0].type,'table');
});
test('nested emphasis, strike, reference links, autolinks and explicit line breaks',()=>{
 const [p]=content('**bold _nested_** ~~gone~~ [reference][r] https://example.com\nnext\n\n[r]: https://example.com/path "title"');
 assert.equal(p.tokens[0].type,'strong');assert.ok(p.tokens[0].tokens.some(t=>t.type==='em'));
 assert.ok(p.tokens.some(t=>t.type==='del'));assert.ok(p.tokens.some(t=>t.type==='link'&&t.title==='title'));assert.ok(p.tokens.some(t=>t.type==='br'));
});
test('fenced code copying retains exact whitespace, longer fences and incomplete streaming content',()=>{
 const code='  const x = "中文";\n\tconsole.log(x);\n\n';
 assert.equal(markdownCode(content('```js\n'+code+'```')[0]),code);
 assert.equal(markdownCode(content('````md\n```js\nx\n```\n````')[0]),'```js\nx\n```\n');
 assert.equal(markdownCode(content('~~~text\r\nlast\r\n')[0]),'last\n');
 assert.equal(markdownCode(content('```text\nlast')[0]),'last');
 assert.equal(markdownCode(content('```text\n```')[0]),'');
 assert.equal(content('    indented code')[0].type,'code');
});
test('HTML remains a text token, dangerous destinations rejected after entity decoding',()=>{
 const [html]=content('<script>window.__unsafe=true</script>');assert.equal(html.type,'html');
 for(const href of ['javascript:alert(1)','java&#x73;cript:alert(1)','data:image/svg+xml,x','//example.com','https://a.test/\nattack'])assert.equal(markdownHref(href),null);
 assert.equal(markdownHref('https://example.com/?a=1&amp;b=2'),'https://example.com/?a=1&b=2');
 assert.equal(markdownText('&lt;img&gt; &amp; &#20013;'),' <img> & 中'.trim());
 const [p]=content('![alt](https://example.com/a.png)');assert.equal(p.tokens[0].type,'image');
});
