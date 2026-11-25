import test from 'node:test';
import assert from 'node:assert/strict';
import { renderUsage } from '../src/usage-view.ts';
import { renderTools } from '../src/view.ts';
import { renderContext } from '../src/context-view.ts';
import { Monitor } from '../src/controller.ts';
import { connectIntegration } from '../src/connect.ts';
import type { Agent } from '../src/types.ts';
const now = Date.parse('2026-11-07T12:00:00Z');
const tool: Agent = { id:'codex', name:'Codex', kind:'cloud', installed:true, running:null, connected:true, status:'ready', message:'', models:[], usage:{inputTokens:10,outputTokens:20,cachedTokens:0,totalTokens:30,spendUsd:null,source:'Saved records',scope:'device',asOf:'2026-11-07T12:00:00Z',periodStart:'2026-10-09',periodEnd:'2026-11-07'}, quotas:[{id:'week',label:'Weekly',remainingPercent:42,resetsAt:'2026-11-08T12:00:00Z',source:'Provider'}], updatedAt:null, capabilities:[] };
const row=(date:string,model:string,totalTokens:number)=>({date,model,totalTokens});
const claude:Agent={...tool,id:'claude',name:'Claude Code',modelHistory:[
  row('2026-11-01','A',1000),row('2026-11-07','A',20),row('2026-10-31','A',50),
  row('2026-10-09','Old',6),row('2026-10-08','Too old',1),row('2026-11-08','Future',1),
  row('2026-10-32','Invalid date',1),row('2026-11-07','B<script>',0),
  row('2026-11-07','Unsafe',Number.MAX_SAFE_INTEGER+1),row('2026-11-07','Fraction',1.5),
  row('2026-11-07','Negative',-1),
]};
const render=(a:Agent,period:7|30=7)=>renderUsage([a],a.id,period,true,new Set([a.id]),now);
test('Codex merges quota, context, and local token usage under one general permission',()=>{
  const codex:Agent={...tool,id:'codex',name:'Codex',
    modelHistory:[{date:'2026-11-07',model:'gpt-5.4',inputTokens:10,outputTokens:20,cachedTokens:0,totalTokens:30}],
    usageHistory:[{date:'2026-11-07',inputTokens:10,outputTokens:20,cachedTokens:0,totalTokens:30,recordedResponses:1}]};
  const deniedTools=renderTools([codex],'quotas',true,new Set(),new Set());
  assert.equal(deniedTools.match(/<article\b/g)?.length,1);
  assert.match(deniedTools,/>Codex<\/h3>/);
  assert.doesNotMatch(deniedTools,/Local token usage|token usage/);
  const deniedUsage=renderUsage([codex],'codex',7,true,new Set(),now);
  assert.match(deniedUsage,/Permission required/);
  assert.doesNotMatch(deniedUsage,/<progress|30 tokens/);

  const approved=new Set(['codex']);
  const tools=renderTools([codex],'quotas',true,approved,new Set());
  assert.match(tools,/data-action="revoke" data-agent="codex"/);
  assert.doesNotMatch(tools,/Local token usage|Connect token usage|Revoke token usage/);
  const detail=renderUsage([codex],'codex',7,true,approved,now);
  assert.match(detail,/aria-pressed="true">Codex<\/button>/);
  assert.match(detail,/>30 tokens</);
  assert.match(detail,/Est\. cost \(USD\)/);
  const total=renderUsage([codex],'total',7,true,approved,now);
  assert.equal(total.match(/>Codex<\/button>/g)?.length,2); // Selector and one agent row.
  assert.match(total,/<td>10<\/td><td>20<\/td>/);
});
test('each model gets one labelled bar with its exact sum for the selected UTC period',()=>{
  const week=render(claude);
  assert.match(week,/2026-11-01 → 2026-11-07/);
  assert.equal(week.match(/<progress\b/g)?.length,2);
  assert.match(week,/>1,020 tokens</);assert.match(week,/>0 tokens</);
  assert.match(week,/aria-label="A" aria-valuetext="1,020 tokens"/);
  assert.match(week,/B&lt;script&gt;/);assert.doesNotMatch(week,/<script>|Old/);
  const month=render(claude,30);
  assert.match(month,/2026-10-09 → 2026-11-07/);
  assert.equal(month.match(/<progress\b/g)?.length,3);
  assert.match(month,/>1,070 tokens</);assert.match(month,/Old/);
  for(const html of [week,month]) assert.doesNotMatch(html,/Too old|Future|Invalid date|Unsafe|Negative|Fraction/);
});
test('an unsafe period sum is not displayed as a rounded or partial model total',()=>{
  const html=render({...claude,modelHistory:[row('2026-11-06','Overflow',Number.MAX_SAFE_INTEGER),row('2026-11-07','Overflow',1)]});
  assert.doesNotMatch(html,/<progress|Overflow/);
});
test('selecting another tool removes Claude data and never charts non-local or partial values',()=>{
  const ollama:Agent={...tool,id:'ollama',name:'Ollama',kind:'local',usage:{...tool.usage!,scope:'account'},modelUsage:[{model:'API total',totalTokens:555}],modelHistory:[row('2026-11-07','Not local',666)]};
  for(const connected of [true,false]){
    const html=renderUsage([claude,{...ollama,connected,historyCached:true}],'ollama',7,true,new Set(['ollama','claude']),now);
    assert.match(html,/data-tool="ollama"[^>]*aria-pressed="true"/);
    assert.match(html,/No local model usage/);
    assert.doesNotMatch(html,/<progress|API total|Not local|1,020|555|666|Weekly|Active sessions|Recorded tokens|Total tokens|data-action=/);
  }
  const opencodeDated:Agent={...tool,id:'opencode',name:'OpenCode',
    usageHistory:[{date:'2026-11-07',inputTokens:15,outputTokens:5,cachedTokens:10,cacheCreationTokens:3,cacheReadTokens:7,totalTokens:30,recordedResponses:2}],
    modelHistory:[{date:'2026-11-07',model:'anthropic:claude-sonnet-4',totalTokens:30,inputTokens:15,outputTokens:5,cachedTokens:10}]};
  const ocHtml=renderUsage([claude,opencodeDated],'opencode',7,true,new Set(['opencode','claude']),now);
  assert.match(ocHtml,/anthropic:claude-sonnet-4/);
  assert.match(ocHtml,/Total tokens<\/dt><dd>30/);
  assert.match(ocHtml,/Write 3 · Read 7/);
  assert.doesNotMatch(ocHtml,/1,020/);
  const partial=render({...claude,modelHistory:[],modelUsage:ollama.modelUsage,partialUsage:{inputTokens:12,cachedTokens:3,source:'Transcript',asOf:'2026-11-07',periodStart:'2026-10-09',periodEnd:'2026-11-07',note:'Input only'}});
  assert.match(partial,/No local model usage/);assert.doesNotMatch(partial,/<progress|API total|Input only|Recorded input|>0 tokens/);
});
test('denied permission, browser preview and a missing offline cache never expose records',()=>{
  for(const connected of [true,false]){
    const hidden=renderUsage([{...claude,connected,historyCached:true}],'claude',7,true,new Set(),now);
    assert.match(hidden,/Permission required/);assert.doesNotMatch(hidden,/<progress|1,020|Saved history/);
  }
  assert.doesNotMatch(renderUsage([claude],'claude',7,false,new Set(['claude']),now),/<progress|1,020/);
  const offline=render({...claude,connected:false,historyCached:false});
  assert.match(offline,/not connected/);assert.doesNotMatch(offline,/<progress|1,020|Saved history/);
});

test('Codex actions and projections stay unified and clean through approval and revoke', async () => {
  const codex: Agent = {...tool, context: {mcps: [], skills: ['quota-context-sentinel'], note: ''},
    quotas: tool.quotas.map(q => ({...q, label: 'quota-sentinel'})),
    modelHistory: [row('2026-11-07', 'gpt-5', 24)],
    usageHistory: [{date: '2026-11-07', inputTokens: 17, outputTokens: 7, cachedTokens: 3, totalTokens: 24, recordedResponses: 1}]};
  const allowed = new Set<string>();
  const calls: unknown[] = [];
  const monitor = new Monitor(true, async (cmd, args) => {
    if (cmd === 'set_permission') {
      calls.push(args);
      const id = String(args?.agentId);
      if (args?.allowed) allowed.add(id); else allowed.delete(id);
    }
    return {agents: [{...codex, monitoringAllowed: allowed.has(codex.id)}], scannedAt: '', platform: '', warnings: []};
  }, () => {});
  await monitor.refresh(true);
  const cards = () => renderTools(monitor.snapshot.agents, 'quotas', true, monitor.approved, new Set(['codex']), now);
  const usage = (id: string) => renderUsage(monitor.snapshot.agents, id, 7, true, monitor.approved, now);
  const context = () => renderContext(monitor.snapshot.agents, 'codex', true, monitor.approved);
  const action = async (label: string, grant: boolean) => {
    const match = [...cards().matchAll(/<button[^>]*data-action="(connect|revoke)" data-agent="([^"]+)"[^>]*>([^<]+)<\/button>/g)].find(m => m[3] === label);
    assert.ok(match, label);
    const id = match[2] as Agent['id'];
    assert.equal(match[1], grant ? 'connect' : 'revoke');
    if (grant) await connectIntegration(id, async () => assert.fail('unexpected login'), () => monitor.permission(id, true));
    else await monitor.permission(id, false);
  };
  assert.equal((cards().match(/<article\b/g) ?? []).length, 1);
  assert.equal((usage('codex').match(/data-section="usage"[^>]*>Codex<\/button>/g) ?? []).length, 1);
  assert.equal((context().match(/data-section="context"[^>]*>Codex<\/button>/g) ?? []).length, 1);
  await action('Connect', true);
  assert.match(cards(), /quota-sentinel/);
  assert.match(context(), /quota-context-sentinel/);
  assert.match(usage('codex'), /gpt-5[\s\S]*24 tokens/);
  assert.match(usage('total'), /2026-11-07: 17 input, 7 output, 3 cache, 24 total/);
  await action('Revoke', false);
  assert.match(usage('codex'), /Permission required/);
  assert.doesNotMatch(cards() + context() + usage('codex') + usage('total'), /quota-sentinel|quota-context-sentinel|gpt-5|24 tokens|17 input/);
  assert.match(usage('total'), /No local usage recorded for this period/);
  assert.deepEqual(calls, [{agentId: 'codex', allowed: true}, {agentId: 'codex', allowed: false}]);
});
