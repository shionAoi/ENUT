import test from 'node:test';
import assert from 'node:assert/strict';
import { renderShell } from '../src/shell.ts';
import { selectedTool } from '../src/usage-view.ts';
import type { Agent } from '../src/types.ts';
test('navigation has Context, EZ Control, and Settings after Usage without principal view switch',()=>{
  const html=renderShell();
  assert.match(html,/data-page="usage"[^]*data-page="context"[^]*data-page="control"[^]*data-page="settings"/);
  assert.doesNotMatch(html,/data-view=/); assert.match(html,/Local runtimes/);
  assert.match(html,/id="usage-content"/); assert.match(html,/id="context-content"/); assert.match(html,/id="control-content"/);
  for(const id of ['theme','hide-window','compact','permission','billing-dialog']) assert.match(html,new RegExp(`id="${id}"`));
});
test('tool selection retains an installed choice and falls back only when absent',()=>{
  const agents=[{id:'codex',installed:true},{id:'hermes',installed:true},{id:'claude',installed:false}] as Agent[];
  assert.equal(selectedTool(agents,'hermes')?.id,'hermes');
  assert.equal(selectedTool(agents,'claude')?.id,'codex');
  assert.equal(selectedTool([],null),null);
});
