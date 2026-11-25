import test from 'node:test';
import assert from 'node:assert/strict';
import { Monitor } from '../src/controller.ts';
import type { Agent, Snapshot } from '../src/types.ts';

test('revocation masks Context and daily model metadata even in late snapshots', async () => {
  const agent: Agent = { id:'codex', name:'Codex', kind:'cloud', installed:true, running:null, connected:true,
    status:'ready', message:'', models:[], usage:null, quotas:[], updatedAt:null, capabilities:[],
    context:{ mcps:[{name:'private-mcp',status:'connected'}],skills:['private-skill'],note:'' },
    modelHistory:[{date:'2026-11-07',model:'private-model',totalTokens:42}] };
  const snapshot: Snapshot = { agents:[agent],scannedAt:'',platform:'test',warnings:[] };
  const monitor = new Monitor(true,async()=>snapshot,()=>{});
  await monitor.refresh();
  assert.equal(monitor.snapshot.agents[0].context?.skills.length,1);
  await monitor.permission('codex',false);
  await monitor.refresh();
  assert.equal(monitor.snapshot.agents[0].context,null);
  assert.deepEqual(monitor.snapshot.agents[0].modelHistory,[]);
  assert.equal(monitor.snapshot.agents[0].connected,false);
});
