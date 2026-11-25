import test from 'node:test';
import assert from 'node:assert/strict';
import { renderTools } from '../src/view.ts';
import type { Agent } from '../src/types.ts';
const agent:Agent={id:'codex',name:'Codex',kind:'cloud',installed:true,running:null,connected:true,status:'ready',message:'Provider reported',models:[],usage:null,quotas:[{id:'session',label:'Session',remainingPercent:62,resetsAt:'2026-11-07T15:08:00Z',source:'Provider'}],updatedAt:'2026-11-07T12:00:00Z',capabilities:['quota']};
test('the help icon explains the connection action without controlling the separate quota details',()=>{
  const tool={...agent,quotas:[...agent.quotas,{...agent.quotas[0],id:'week',label:'Weekly',remainingPercent:99.99}]};
  for(const open of [false,true]){
    const html=renderTools([tool],'quotas',true,new Set(['codex']),new Set(open?['codex']:[]),Date.parse('2026-11-07T12:00:00Z'));
    const [summary,details]=html.split('id="detail-codex"');
    assert.match(summary,/Session<[^]*?62% left/);assert.match(summary,/Weekly<[^]*?99.99% left/);
    assert.doesNotMatch(summary,/3h 8m|Source: Provider|usage-grid/);
    const help=summary.match(/<button[^>]*>\?<\/button>/)?.[0]??'';
    assert.match(help,/aria-describedby="help-codex"/);
    assert.doesNotMatch(help,/data-action|aria-expanded/);
    assert.match(summary,/<span[^>]*id="help-codex"[^>]*role="tooltip"[^>]*>Revoke removes/);
    const button=summary.match(/<button[^>]*data-action="details"[^>]*>Quota details<\/button>/)?.[0]??'';
    assert.match(button,/type="button"/);assert.match(button,/data-agent="codex"/);
    assert.match(button,/aria-label="[^"]*Codex[^"]*"/);assert.match(button,/aria-controls="detail-codex"/);
    assert.match(button,new RegExp(`aria-expanded="${open}"`));
    if(open){assert.match(details,/3h 8m/);assert.match(details,/Source: Provider/);}
    else assert.match(details,/^[^>]*hidden/);
    assert.doesNotMatch(details,/role="tooltip"|Connect grants/);
  }
});
test('expanded quota details show a ring and reset date for each reported window without the extra footer',()=>{
  const weekly={...agent.quotas[0],id:'week',label:'Weekly',remainingPercent:92,resetsAt:'2026-11-12T16:34:00Z'};
  const tools:Agent[]=[{...agent,updatedAt:null,quotas:[agent.quotas[0],weekly]},
    {...agent,id:'claude',name:'Claude',quotas:[{...weekly,id:'unknown',label:'Unknown window',remainingPercent:null,resetsAt:null}]}];
  const html=renderTools(tools,'quotas',true,new Set(['codex','claude']),new Set(['codex','claude']),Date.parse('2026-11-07T12:00:00Z'));
  const [,codex,claude]=html.split(/id="detail-(?:codex|claude)"/);
  assert.equal((codex.match(/class="quota-ring"/g)??[]).length,2);
  assert.match(codex,/62% remaining[\s\S]*<time datetime="2026-11-07T15:08:00Z">/);
  assert.match(codex,/92% remaining[\s\S]*<time datetime="2026-11-12T16:34:00Z">/);
  assert.match(claude,/Unknown window[\s\S]*Allowance unavailable[\s\S]*Reset: Unavailable/);
  assert.doesNotMatch(claude,/class="quota-ring"|<time\b/);
  assert.doesNotMatch(html,/<progress\b|Provider reported|remembered monitoring access|Not updated/);
});
test('unknown allowance is not a full or empty bucket; actual zero is still visible',()=>{
  for(const n of [null,-1,101,0,0.01,100]){
    const html=renderTools([{...agent,quotas:[{...agent.quotas[0],remainingPercent:n}]}],'quotas',true,new Set(['codex']),new Set()).split('id="detail-codex"')[0];
    if(n===null||n<0||n>100){assert.match(html,/Session<[^]*?Unavailable/);assert.doesNotMatch(html,/%/);}
    else assert.ok(html.includes(`${n}% left`));
  }
});
test('quota labels round to two decimal places without rounding the meter value',()=>{
  const tool={...agent,quotas:[{...agent.quotas[0],remainingPercent:62.3456}]};
  const html=renderTools([tool],'quotas',true,new Set(['codex']),new Set(['codex']));
  assert.match(html,/62\.35% left/);
  assert.match(html,/aria-valuetext="62\.35% remaining"/);
  assert.match(html,/>62\.35% remaining<\/strong>/);
  assert.match(html,/value="62\.3456"/);
});
test('principal rows and expanded details show quotas only even for legacy view arguments',()=>{
  const withUsage={...agent,usage:{inputTokens:111,outputTokens:0,cachedTokens:0,totalTokens:111,spendUsd:null,source:'Private usage',scope:'device' as const,asOf:'2026-11-07'},models:[{id:'private-model',name:'private-model',loaded:true,sizeBytes:null}]};
  for(const view of ['usage','models','quotas'] as const){
    const html=renderTools([withUsage],view,true,new Set(['codex']),new Set(['codex']));
    assert.match(html,/62% remaining/); assert.doesNotMatch(html,/111|private-model|Private usage|usage-grid/);
  }
  const local=renderTools([{...agent,id:'ollama',kind:'local',quotas:[],models:withUsage.models}],'models',true,new Set(['ollama']),new Set());
  assert.match(local,/No subscription quota/); assert.doesNotMatch(local,/reported models/);
  const disconnected=renderTools([{...withUsage,connected:false}],'quotas',true,new Set(['codex']),new Set(['codex']));
   assert.doesNotMatch(disconnected,/62%|Private usage|Provider reported/);
  const zero=renderTools([{...agent,kind:'local',quotas:[{...agent.quotas[0],remainingPercent:0}]}],'quotas',true,new Set(['codex']),new Set(['codex']));
  assert.match(zero,/0% remaining/); assert.match(zero,/value="0"/);
});
test('toolbox keeps connection and detail actions accessible but hides unapproved metadata',()=>{
  const html=renderTools([{...agent,name:'<unsafe>',installed:false},agent],'quotas',true,new Set(),new Set(['codex']));
  assert.doesNotMatch(html,/unsafe|62%/);
  assert.match(html,/data-action="connect"/);
  assert.match(html,/Permission required/);
  const connected=renderTools([agent],'quotas',true,new Set(['codex']),new Set(['codex']));
  assert.match(connected,/aria-expanded="true"/);
  assert.match(connected,/62% remaining/);
  assert.match(connected,/data-action="revoke"/);
});
