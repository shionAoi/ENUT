import test from 'node:test';
import assert from 'node:assert/strict';
import {presentAgent} from '../src/presentation.ts';
import {renderTools,quotas} from '../src/view.ts';
import {renderUsage,toolButtons} from '../src/usage-view.ts';
import {renderContext} from '../src/context-view.ts';
import type {Agent,Quota} from '../src/types.ts';
const quota=(group:string,window:string,n:number):Quota=>({id:`${group}:${window}`,label:`${group} · Shared allowance · ${window}`,remainingPercent:n,resetsAt:'2026-11-12T10:00:00Z',source:'Antigravity native quota'});
const agent:Agent={id:'antigravity',name:'Antigravity',kind:'cloud',installed:true,running:null,connected:true,status:'ready',message:'',models:[],usage:null,quotas:[quota('Claude','weekly',1),quota('Gemini','five_hour',65),quota('Gemini','weekly',80),quota('Gemini','monthly',3),quota('GPT','5h',2)],updatedAt:null,capabilities:[]};
test('Gemini alias keeps native identity and every reported quota unchanged',()=>{
 const before=JSON.stringify(agent),p=presentAgent(agent);
 assert.equal(p.id,'antigravity');assert.equal(p.name,'Gemini');
 assert.deepEqual(p.quotas,agent.quotas);
 assert.equal(JSON.stringify(agent),before);assert.deepEqual(presentAgent(p),p);
});
test('Tools shows native groups and compact reported windows; sibling tabs keep the alias but not quota data',()=>{
 const approved=new Set(['antigravity']);const html=renderTools([agent],'quotas',true,approved,new Set()).split('id="detail-antigravity"')[0];
 assert.match(html,/>Gemini<\/h3>/);
 const headings=[...html.matchAll(/<h4[^>]*>(.*?)<\/h4>/g)].map(m=>m[1]);
 assert.deepEqual(headings,['Claude','Gemini','GPT']);
 const text=html.replace(/<[^>]+>/g,' ');
 assert.match(text,/Claude\s+Weekly limit\s+1% left/);
 assert.match(text,/Gemini\s+5h limit\s+65% left\s+Weekly limit\s+80% left\s+monthly\s+3% left/);
 assert.match(text,/GPT\s+5h limit\s+2% left/);
 const usage=renderUsage([agent],'antigravity',7,true,approved,Date.parse('2026-11-11T12:00:00Z'));
 assert.doesNotMatch(usage,/65% remaining|80% remaining|Quota & resets|GPT|monthly/);
 assert.match(toolButtons([agent],'antigravity','context'),/>Gemini<\/button>/);
 assert.match(renderContext([agent],'antigravity',true,approved),/>Gemini<\/h3>/);
});
test('empty native quotas do not produce synthetic groups or allowances',()=>{
 const p=presentAgent({...agent,quotas:[]});
 assert.match(quotas(p),/Allowance unavailable/);
 const html=renderTools([p],'quotas',true,new Set(['antigravity']),new Set());assert.match(html,/Quota unavailable/);assert.doesNotMatch(html,/% left/);
});
test('overlapping windows distinguish bucket names and stable IDs when names also collide',()=>{
 const buckets=['burst-a','burst-b','slow'].map((id,i)=>({...quota('Pro <&>',i===2?'five_hour':'5h',50),id,label:`Pro <&> · ${i===2?'Slow':'Fast'} · ${i===2?'five_hour':'5h'}`}));
 for(const list of [buckets,[...buckets].reverse()]){
  const summary=renderTools([{...agent,quotas:list}],'quotas',true,new Set([agent.id]),new Set()).split('id="detail-antigravity"')[0];
  assert.match(summary,/Pro &lt;&amp;&gt;/);
   for(const id of ['burst-a','burst-b'])assert.match(summary,new RegExp(`5h limit · Fast · ${id}</`));
   assert.match(summary,/5h limit · Slow</);
 }
});
test('recognized native window spellings preserve zero and unknown values',()=>{
 for(const window of ['5h','5-hour','5 hours','FIVE_HOURS','five_hour','weekly','WEEKLY']){
  const p=presentAgent({...agent,quotas:[quota('Gemini models',window,0)]});assert.equal(p.quotas.length,1);assert.equal(p.quotas[0].remainingPercent,0);
 }
 const q=quota('Gemini','weekly',0);q.remainingPercent=null;assert.equal(presentAgent({...agent,quotas:[q]}).quotas[0].remainingPercent,null);
});
test('sibling integrations retain the exact same object, fields and quota behavior',()=>{
 const codex:Agent={...agent,id:'codex',name:'Codex'};assert.equal(presentAgent(codex),codex);
 assert.equal(presentAgent({...agent,id:'hermes',name:'Hermes'}).quotas.length,5);
});
