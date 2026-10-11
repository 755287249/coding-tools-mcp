import {readFileSync,writeFileSync} from 'node:fs';
const root=new URL('./',import.meta.url),target=new URL('../../static/plugins/coderabbit-seeds.user.js',root);
const header=`// ==UserScript==
// @name         Coding Tools MCP 种子库
// @namespace    https://github.com/755287249/coding-tools-mcp
// @version      1.1.1
// @description  在当前 CodeRabbit 账号下批量创建并接入 MCP 项目种子库
// @match        https://app.coderabbit.ai/*
// @run-at       document-start
// @grant        unsafeWindow
// @grant        GM_xmlhttpRequest
// @grant        GM_getValue
// @grant        GM_setValue
// @grant        GM_registerMenuCommand
// @connect      *
// ==/UserScript==
`;
const result=header+"\n(function(){'use strict';\n"+readFileSync(new URL('context.mjs',root),'utf8').replace(/^export /gm,'')+'\n'+readFileSync(new URL('core.mjs',root),'utf8').replace(/^export /gm,'')+'\n'+readFileSync(new URL('browser.js',root),'utf8')+'\n})();\n';
if(process.argv.includes('--check')){if(readFileSync(target,'utf8')!==result)throw Error('Userscript needs regeneration');}else writeFileSync(target,result);
