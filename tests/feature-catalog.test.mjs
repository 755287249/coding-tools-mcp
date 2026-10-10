import test from 'node:test';
import assert from 'node:assert/strict';
import { featureRows, filterFeatureRows } from '../src/lib/workspace/feature-catalog.ts';
const server = (key, extra = {}) => ({key, name:key, provider:'Claude', scope:'workspace', sourcePath:'.mcp.json', selected:true, enabled:false, supported:true, sourceEnabled:true, connected:false, transport:'http', toolCount:3, error:null, ...extra});
test('catalog preserves selection while master is off and excludes connection secrets', () => {
 const extensions={mcpActive:false,mcpServers:[server('Docs',{command:'private command',url:'https://private.invalid/?secret=value'})]};
 const before=JSON.stringify(extensions);
 const [row]=featureRows('mcp',null,extensions);
 assert.equal(row.selected,true); assert.equal(row.enabled,false);
 assert.equal(row.toolCount,3);assert.equal(row.connected,false);
 assert.equal(JSON.stringify(row).includes('private'),false);
 assert.equal(JSON.stringify(extensions),before);
});
test('unsupported and source-disabled configurations retain distinct statuses', () => {
 const rows=featureRows('mcp',null,{mcpServers:[server('Unsupported',{supported:false}),server('Source off',{sourceEnabled:false,error:'Unavailable'})]});
 assert.equal(rows[0].supported,false);assert.equal(rows[0].sourceEnabled,true);
 assert.equal(rows[1].supported,true);assert.equal(rows[1].sourceEnabled,false);assert.equal(rows[1].error,'Unavailable');
});
test('scope and multiword search combine without mutating inventory', () => {
 const rows=featureRows('mcp',null,{mcpServers:[server('Docs'),server('Docs personal',{scope:'user'}),server('Local Docs',{scope:'local'}),server('Database',{provider:'Codex'})]});
 assert.deepEqual(filterFeatureRows(rows,'DOCS claude','workspace').map(r=>r.key),['Docs','Local Docs']);
 assert.deepEqual(filterFeatureRows(rows,' docs ','user').map(r=>r.key),['Docs personal']);
 assert.equal(filterFeatureRows(rows,'missing','all').length,0);assert.equal(rows.length,4);
});
test('skills, hooks and unloaded inventories normalize without mixing kinds', () => {
 assert.deepEqual(featureRows('mcp',null,null),[]);
 const [skill]=featureRows('skills',{skills:[{key:'s',name:'Planning',description:'Plan changes',source:'agents',scope:'user',relativePath:'.agents/skills/plan/SKILL.md',version:'1',selected:true,enabled:true}]},null);
 assert.equal(skill.kind,'skill');assert.equal(skill.path,'.agents/skills/plan/SKILL.md');assert.equal(skill.version,'1');
 const [hook]=featureRows('hooks',null,{hooks:[{key:'h',event:'PreToolUse',matcher:'Bash',provider:'Claude',scope:'local',sourcePath:'.claude/settings.json',handlerType:'command',selected:false,enabled:false,supported:false,sourceEnabled:true}]});
 assert.equal(hook.kind,'hook');assert.equal(hook.description,'Bash');assert.equal(hook.selected,false);assert.equal(hook.supported,false);
});
