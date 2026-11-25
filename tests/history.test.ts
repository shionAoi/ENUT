import test from 'node:test';
import assert from 'node:assert/strict';
import { renderHistory, renderAgents } from '../src/view.ts';
import type { Agent } from '../src/types.ts';
const agent:Agent={id:'gemini',name:'Gemini CLI',kind:'cloud',installed:true,running:null,connected:true,status:'ready',message:'',models:[],usage:null,quotas:[],updatedAt:null,capabilities:[]};

test('history uses reported day totals, not invented missing dates or unapproved records',()=>{
  const withHistory={...agent,usageHistory:[{date:'2026-11-06',inputTokens:80,outputTokens:20,cachedTokens:10,totalTokens:100},{date:'2026-11-07',inputTokens:20,outputTokens:10,cachedTokens:0,totalTokens:30}]};
  const html=renderHistory([withHistory]);
  assert.match(html,/100 tokens/);
  assert.match(html,/30 tokens/);
  assert.match(html,/not zero/);
  assert.doesNotMatch(renderHistory([{...withHistory,connected:false}]),/100 tokens/);
});

test('Claude partial input never becomes a complete output or total count',()=>{
  const claude:Agent={...agent,id:'claude',name:'Claude Code',partialUsage:{inputTokens:60,cachedTokens:20,source:'Observed input only',asOf:'2026-11-07T12:00:00Z',periodStart:'2026-10-09T00:00:00Z',periodEnd:'2026-11-07T12:00:00Z',note:'Output unknown'}};
  const html=renderAgents([claude],'usage',true,new Set(['claude']));
  assert.match(html,/60/);
  assert.match(html,/Output/);
  assert.match(html,/Unavailable/);
  assert.doesNotMatch(html,/Total tokens<\/dt><dd>60/);
  assert.doesNotMatch(renderAgents([{...claude,connected:false}],'usage',true,new Set()),/60/);
});
