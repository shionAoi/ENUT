import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {renderShell} from '../src/shell.ts';
import {renderTools} from '../src/view.ts';

test('header and favicon share the app mark',()=>{
 const favicon=readFileSync(new URL('../index.html',import.meta.url),'utf8').match(/<link[^>]*rel="icon"[^>]*href="([^"]+)"/)?.[1];
 assert.equal(favicon,'/app-icon.svg');
 assert.ok(renderShell().includes(`src="${favicon}"`));
});

test('EZTYN AI MONITOR names the shell, preview, document and desktop window',()=>{
 assert.match(renderShell(),/<h1>EZTYN AI MONITOR<\/h1>/);
 assert.match(renderTools([],'quotas',false,new Set(),new Set()),/Open EZTYN AI MONITOR as a desktop app/);
 assert.match(readFileSync(new URL('../index.html',import.meta.url),'utf8'),/<title>EZTYN AI MONITOR<\/title>/);
 const config=JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json',import.meta.url),'utf8'));
 assert.equal(config.productName,'EZTYN AI MONITOR');
 assert.equal(config.app.windows[0].title,'EZTYN AI MONITOR');
 assert.equal(config.identifier,'com.local.aiusagemonitor');
});
test('current source and documentation do not retain old display branding',()=>{
 for(const path of ['README.md','docs/ipc-contract.md','docs/integrations/authentication.md','src-tauri/src/main.rs','src-tauri/src/panel.rs','src-tauri/src/codex.rs']){
  const text=readFileSync(new URL(`../${path}`,import.meta.url),'utf8');
  assert.doesNotMatch(text,/AI Usage Monitor|EXTYN AI MONITOR/,path);
  assert.match(text,/EZTYN AI MONITOR/,path);
 }
});
