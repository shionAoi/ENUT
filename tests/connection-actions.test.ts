import test from 'node:test';
import assert from 'node:assert/strict';
import {connectionAction,connectionHint} from '../src/connection-actions.ts';
import {renderTools} from '../src/view.ts';
import type {Agent} from '../src/types.ts';
const agent:Agent={id:'codex',name:'Codex',kind:'cloud',installed:true,running:null,connected:false,status:'error',message:'Timeout',models:[],usage:null,quotas:[],updatedAt:null,capabilities:[]};
test('only explicit native required evidence exposes Login',()=>{
 for(const authentication of [undefined,null,'unknown','available','notRequired'] as const){
  assert.deepEqual(connectionAction({...agent,authentication},new Set()),{label:'Connect',action:'connect'});
 }
 assert.deepEqual(connectionAction({...agent,authentication:'required'},new Set(['codex'])),{label:'Login',action:'login'});
 assert.match(connectionHint(agent),/permission.*native.*saved login/i);
 assert.match(connectionHint({...agent,authentication:'required'}),/device|manual/);
 assert.match(connectionHint({...agent,authentication:'notRequired'}),/no provider login/i);
});
test('connected permission retains Revoke; unavailable permission retains secondary revocation',()=>{
 const approved=new Set(['codex']);
 assert.deepEqual(connectionAction({...agent,connected:true},approved),{label:'Revoke',action:'revoke'});
 assert.deepEqual(connectionAction({...agent,connected:true},new Set()),{label:'Connect',action:'connect'});
 const html=renderTools([agent],'quotas',true,approved,new Set(['codex']));
 assert.match(html,/data-action="connect"/);assert.match(html,/data-action="revoke"/);
 assert.match(html,/native.*saved login/i);
 const required=renderTools([{...agent,authentication:'required'}],'quotas',true,approved,new Set());
 assert.match(required,/data-action="login"[^>]*>Login/);
 assert.doesNotMatch(required,/data-action="connect"|Sign in/);
});
