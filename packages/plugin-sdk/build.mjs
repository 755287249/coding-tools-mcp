#!/usr/bin/env node
import { build } from 'vite';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export async function buildPlugin(directory = process.cwd()) {
  const root = path.resolve(directory), manifest = JSON.parse(await readFile(path.join(root, 'manifest.json'), 'utf8'));
  if (!/^[a-z][a-z0-9-]{1,63}$/.test(manifest.id)) throw new Error('manifest.id must match [a-z][a-z0-9-]{1,63}');
  const sdkEntry = path.join(path.dirname(fileURLToPath(import.meta.url)), 'src/index.ts');
  const result = await build({ root, configFile: false, logLevel: 'warn', resolve: { alias: { '@coding-tools/plugin-sdk': sdkEntry } }, build: { write: false, minify: 'oxc', target: 'es2022', cssCodeSplit: false,
    lib: { entry: path.join(root, 'src/index.ts'), formats: ['iife'], name: 'CodingToolsPlugin' } } });
  const outputs = Array.isArray(result) ? result.flatMap(item => item.output) : result.output;
  const js = outputs.find(item => item.type === 'chunk'), css = outputs.find(item => item.type === 'asset' && item.fileName.endsWith('.css'));
  if (!js || js.type !== 'chunk') throw new Error('Vite did not emit plugin JavaScript');
  const style = css && css.type === 'asset' ? `(()=>{const s=document.createElement('style');s.textContent=${JSON.stringify(String(css.source))};document.head.append(s)})();\n` : '';
  const output = { format: 1, manifest, code: style + js.code };
  const dist = path.join(root, 'dist'); await mkdir(dist, { recursive: true });
  const target = path.join(dist, `${manifest.id}.ctplugin.json`); await writeFile(target, JSON.stringify(output)); return target;
}

if (process.argv[1] && path.resolve(process.argv[1]) === new URL(import.meta.url).pathname) console.log(await buildPlugin(process.argv[2]));
