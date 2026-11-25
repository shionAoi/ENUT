import test from 'node:test';
import assert from 'node:assert/strict';
import { Monitor } from '../src/controller.ts';
import { renderAgents, renderHistory, renderSummary } from '../src/view.ts';
import type { Agent, Snapshot } from '../src/types.ts';
const agent: Agent = {id:'claude',name:'Claude',kind:'cloud',installed:true,running:null,connected:true,status:'ready',message:'',models:[],usage:null,quotas:[],updatedAt:null,capabilities:[],usageHistory:[{date:'2026-11-07',inputTokens:10,outputTokens:2,cachedTokens:3,totalTokens:12}],modelUsage:[{model:'fixture',totalTokens:12}],partialUsage:{inputTokens:60,cachedTokens:20,source:'synthetic',asOf:'2026-11-07T12:00:00Z',periodStart:'2026-10-09T00:00:00Z',periodEnd:'2026-11-07T12:00:00Z',note:'Output unknown'}};
const snapshot: Snapshot = {agents:[agent],scannedAt:'',platform:'fixture',warnings:[]};
test('late snapshots cannot republish new revoked fields',async()=>{
  let finish!: (value: Snapshot)=>void;
  const monitor=new Monitor(true,async(command)=>command==='refresh_usage'?new Promise<Snapshot>(r=>finish=r):snapshot,()=>{});
  monitor.snapshot=snapshot;monitor.approved.add('claude');
  const refresh=monitor.refresh();const revoke=monitor.permission('claude',false);
  for(const a of monitor.snapshot.agents){assert.equal(a.partialUsage,null);assert.deepEqual(a.usageHistory,[]);assert.deepEqual(a.modelUsage,[]);}
  finish(snapshot);await Promise.all([refresh,revoke]);
  const a=monitor.snapshot.agents[0];assert.equal(a.partialUsage,null);assert.deepEqual(a.usageHistory,[]);assert.deepEqual(a.modelUsage,[]);assert.equal(a.connected,false);
});
test('partial counters never become complete totals or duplicate cache',()=>{
  const a={...agent,usageHistory:[]};const html=renderSummary([a])+renderAgents([a],'usage',true,new Set(['claude']));
  assert.match(html,/Observed input tokens/);assert.match(html,/60/);assert.doesNotMatch(html,/>80</);assert.match(html,/Total tokens<\/dt><dd>Unavailable/);assert.match(renderHistory([a]),/No recorded token history yet/);
  const hidden=renderAgents([{...a,connected:false}],'usage',true,new Set());assert.doesNotMatch(hidden,/Recorded input/);
});
