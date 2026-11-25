import test from 'node:test';
import assert from 'node:assert/strict';
import {Monitor} from '../src/controller.ts';
import {renderUsage} from '../src/usage-view.ts';
import type {Agent, Snapshot} from '../src/types.ts';

const agent = {
 id:'claude',name:'Claude Code',kind:'cloud',installed:true,running:null,connected:false,
 monitoringAllowed:true,historyCached:true,status:'unavailable',message:'Offline',
 models:[],quotas:[],capabilities:[],updatedAt:'2026-11-12T12:00:00Z',
 usage:{inputTokens:10,outputTokens:5,cachedTokens:0,totalTokens:15,spendUsd:null,scope:'device',source:'Saved records',asOf:'2026-11-12T12:00:00Z'},
 modelHistory:[{date:'2026-11-12',model:'test-model',totalTokens:15}],
} satisfies Agent;
const snapshot:Snapshot={agents:[agent],platform:'test',warnings:[],scannedAt:'2026-11-13T12:00:00Z'};
test('restart restores explicit approval even when native status is offline',async()=>{
 const monitor=new Monitor(true,async()=>snapshot,()=>{});
 await monitor.refresh(true);
 assert.equal(monitor.approved.has('claude'),true);
 assert.equal(monitor.snapshot.agents[0].connected,false);
});
test('approved cached history renders offline without claiming active sessions',()=>{
 const html=renderUsage([agent],'claude',7,true,new Set(['claude']),Date.parse(snapshot.scannedAt));
 assert.match(html,/test-model/);
 assert.match(html,/Saved history/);
 assert.match(html,/15 tokens/);
 assert.match(html,/2026-11-07 → 2026-11-13/);
 assert.doesNotMatch(html,/Active sessions|\d+ running/);
 const hidden=renderUsage([agent],'claude',7,true,new Set(),Date.parse(snapshot.scannedAt));
 assert.doesNotMatch(hidden,/test-model/);
 assert.match(hidden,/Permission required/);
});
test('login pause only masks runtime state and never invokes destructive Revoke',async()=>{
 const commands:string[]=[];
 const monitor=new Monitor(true,async command=>{commands.push(command);return snapshot;},()=>{});
 await monitor.refresh(true);
 await monitor.pause('claude');
 assert.equal(monitor.approved.has('claude'),false);
 assert.equal(monitor.snapshot.agents[0].usage,null);
 assert.deepEqual(commands,['scan_agents']);
 await monitor.permission('claude',true);
 assert.equal(monitor.approved.has('claude'),true);
 assert.equal(monitor.snapshot.agents[0].modelHistory?.[0].totalTokens,15);
});
