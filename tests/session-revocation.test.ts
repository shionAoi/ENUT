import test from 'node:test';
import assert from 'node:assert/strict';
import {Monitor} from '../src/controller.ts';
import type {Agent,Snapshot} from '../src/types.ts';

test('revocation clears active-session and authentication evidence including late snapshots',async()=>{
 const agent:Agent={id:'opencode',name:'OpenCode',kind:'cloud',installed:true,running:true,connected:true,status:'ready',message:'',models:[],usage:null,quotas:[],updatedAt:null,capabilities:[],authentication:'available',activeSessions:{sessions:[{id:'synthetic',state:'running'}],source:'Native fixture',scope:'One service',asOf:'2026-11-11T12:00:00Z'}};
 const snapshot:Snapshot={agents:[agent],scannedAt:'',platform:'test',warnings:[]};
 const monitor=new Monitor(true,async()=>snapshot,()=>{});
 await monitor.refresh();assert.equal(monitor.snapshot.agents[0].activeSessions?.sessions.length,1);
 await monitor.permission('opencode',false);await monitor.refresh();
 assert.equal(monitor.snapshot.agents[0].activeSessions,null);
 assert.equal(monitor.snapshot.agents[0].authentication,null);
 assert.equal(monitor.snapshot.agents[0].connected,false);
});

test('an IPC refresh failure discards live session and login evidence, not recorded history',async()=>{
 let fail=false;
 const agent:Agent={id:'opencode',name:'OpenCode',kind:'cloud',installed:true,running:true,connected:true,status:'ready',message:'',models:[],usage:null,quotas:[],updatedAt:null,capabilities:[],authentication:'available',activeSessions:{sessions:[{id:'synthetic',state:'running'}],source:'Native fixture',scope:'One service',asOf:'2026-11-11T12:00:00Z'},modelHistory:[{date:'2026-11-11',model:'fixture-model',totalTokens:12}]};
 const snapshot:Snapshot={agents:[agent],scannedAt:'',platform:'test',warnings:[]};
 const monitor=new Monitor(true,async()=>{if(fail)throw Error('fixture transport failed');return snapshot;},()=>{});
 await monitor.refresh();fail=true;await monitor.refresh();
 assert.equal(monitor.snapshot.agents[0].activeSessions,null);
 assert.equal(monitor.snapshot.agents[0].authentication,null);
 assert.deepEqual(monitor.snapshot.agents[0].modelHistory,agent.modelHistory);
 assert.match(monitor.notice,/Could not update/);
});
