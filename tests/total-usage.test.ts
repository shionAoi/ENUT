import test from 'node:test';
import assert from 'node:assert/strict';
import { renderUsage } from '../src/usage-view.ts';
import type { Agent } from '../src/types.ts';

const now = Date.parse('2026-11-21T12:00:00Z');
const claude: Agent = {
  id: 'claude', name: 'Claude Code', kind: 'cloud', installed: true, connected: false, running: null,
  status: 'unavailable', message: '', historyCached: true, models: [], quotas: [], usage: null, updatedAt: null, capabilities: [],
  usageHistory: [
    {date:'2026-11-21', inputTokens:31, outputTokens:9, cachedTokens:7, cacheCreationTokens:2, cacheReadTokens:5, totalTokens:47, recordedResponses:2},
    {date:'2026-09-01', inputTokens:70, outputTokens:0, cachedTokens:0, totalTokens:70},
  ],
  modelHistory: [
    {date:'2026-11-21', model:'<local-model>', totalTokens:47, inputTokens:31, outputTokens:9, cachedTokens:7},
    {date:'2026-09-01', model:'older-model', totalTokens:70},
  ],
};

test('Total Usage leads the selector and charts approved dated local data for the chosen range', () => {
  const approved = new Set(['claude']);
  const week = renderUsage([claude], 'total', 7, true, approved, now);
  assert.match(week, /data-tool="total"[^>]*aria-pressed="true"[^>]*>TOTAL USAGE/);
  assert.ok(week.indexOf('TOTAL USAGE') < week.indexOf('Claude Code'));
  assert.match(week, /Total tokens<\/dt><dd>47/);
  assert.match(week, /Input tokens<\/dt><dd>31/);
  assert.match(week, /Output tokens<\/dt><dd>9/);
  assert.match(week, /Cache tokens<\/dt><dd>7/);
  assert.match(week, /Est\. cost \(USD\)<\/dt><dd><span aria-label="Unavailable">—<\/span>/);
  assert.match(week, /Write 2 · Read 5/);
  assert.match(week, /Tokens by day/);
  assert.match(week, /&lt;local-model&gt;/);
  assert.match(week, /data-usage-model="&lt;local-model&gt;"[^>]*>&lt;local-model&gt;<small>Claude Code<\/small><\/button><\/th><td>31<\/td><td>9<\/td><td>7<\/td><td>47<\/td><td><span aria-label="Unavailable">—<\/span><\/td>/);
  assert.match(week, /Recorded responses<\/dt><dd>2/);
  assert.match(week, /data-tool="claude"[^>]*>Claude Code/);
  assert.doesNotMatch(week, /older-model|when they work/i);
  const all = renderUsage([claude], 'total', 'all', true, approved, now);
  assert.match(all, /Input tokens<\/dt><dd>101/);
  assert.match(all, /Total tokens<\/dt><dd>117/);
  assert.match(all, /older-model/);
  assert.match(all, /older-model[^]*?<td><span aria-label="Unavailable">—<\/span><\/td>/);
  assert.match(all, /Response count is partial/);
  assert.match(all, /All retained data/);
});

test('Total Usage never substitutes unapproved, non-dated, or partial values for local history', () => {
  const source = {...claude, id:'opencode', name:'OpenCode', connected:true, usageHistory:[], modelHistory:[],
    modelUsage:[{model:'source-period-model',totalTokens:99999}]} as const;
  const html = renderUsage([claude, {...source, modelUsage:[...source.modelUsage], usageHistory:[], modelHistory:[]}],
    'total', 30, true, new Set(['opencode']), now);
  assert.match(html, /No local usage recorded/);
  assert.doesNotMatch(html, /local-model|source-period-model|99,999|Input tokens<\/dt><dd>0/);
});

test('All six ccusage local agents add to one daily total only with separate local grants', () => {
  const gemini: Agent = {...claude, id:'gemini', name:'Gemini CLI', connected:true, historyCached:false,
    usageHistory:[{date:'2026-11-21', inputTokens:100, outputTokens:20, cachedTokens:30, totalTokens:150, recordedResponses:1}],
    modelHistory:[{date:'2026-11-21', model:'gemini-fixture', totalTokens:150, inputTokens:100, outputTokens:20, cachedTokens:30}],
  };
  const opencode: Agent = {...gemini, id:'opencode', name:'OpenCode',
    usageHistory:[{date:'2026-11-21', inputTokens:10, outputTokens:5, cachedTokens:4, totalTokens:19, recordedResponses:1}],
    modelHistory:[{date:'2026-11-21', model:'opencode-model', totalTokens:19, inputTokens:10, outputTokens:5, cachedTokens:4}],
  };
  const antigravity: Agent = {...gemini, id:'antigravity', name:'Gemini',
    usageHistory:[{date:'2026-11-21', inputTokens:20, outputTokens:8, cachedTokens:2, totalTokens:30, recordedResponses:1}],
    modelHistory:[{date:'2026-11-21', model:'claude-opus-4-6', totalTokens:30, inputTokens:20, outputTokens:8, cachedTokens:2}],
  };
  const hermes: Agent = {...gemini, id:'hermes', name:'Hermes',
    usageHistory:[{date:'2026-11-21', inputTokens:5, outputTokens:3, cachedTokens:1, totalTokens:9, recordedResponses:1}],
    modelHistory:[{date:'2026-11-21', model:'hermes-model', totalTokens:9, inputTokens:5, outputTokens:3, cachedTokens:1}],
  };
  const allApproved = new Set(['claude', 'gemini', 'opencode', 'antigravity', 'hermes']);
  const html = renderUsage([claude, gemini, opencode, antigravity, hermes], 'total', 30, true, allApproved, now);
  assert.match(html, /Input tokens<\/dt><dd>166/);
  assert.match(html, /Output tokens<\/dt><dd>45/);
  assert.match(html, /Cache tokens<\/dt><dd>44/);
  assert.match(html, /Total tokens<\/dt><dd>255/);
  assert.match(html, /Est\. cost \(USD\)<\/dt><dd>\$0\*/);
  assert.match(html, /Recorded responses<\/dt><dd>6/);
  assert.match(html, /2026-11-21: 166 input, 45 output, 44 cache, 255 total, ~\$0\*/);
  const revoked = renderUsage([claude, gemini, opencode, antigravity, hermes], 'total', 30, true, new Set(['claude']), now);
  assert.doesNotMatch(revoked, /gemini-fixture|opencode-model|claude-opus-4-6|hermes-model/);
  assert.match(revoked, /Input tokens<\/dt><dd>31/);
});

test('Total Usage filters Tokens by day chart when a model is selected and ignores absent selections', () => {
  const codex: Agent = {
    ...claude,
    id: 'codex',
    name: 'Codex',
    connected: true,
    historyCached: false,
    usageHistory: [
      { date: '2026-11-20', inputTokens: 1_000_000, outputTokens: 100_000, cachedTokens: 0, totalTokens: 1_100_000, recordedResponses: 1 },
      { date: '2026-11-21', inputTokens: 500_000, outputTokens: 50_000, cachedTokens: 0, totalTokens: 550_000, recordedResponses: 1 },
    ],
    modelHistory: [
      { date: '2026-11-20', model: 'gpt-5.4', inputTokens: 1_000_000, outputTokens: 100_000, cachedTokens: 0, totalTokens: 1_100_000 },
      { date: '2026-11-21', model: 'o3-mini', inputTokens: 500_000, outputTokens: 50_000, cachedTokens: 0, totalTokens: 550_000 },
    ],
  };
  const approved = new Set(['claude', 'codex']);
  const filtered = renderUsage([claude, codex], 'total', 7, true, approved, now, 'en', { agentId: 'codex', model: 'gpt-5.4' });
  assert.match(filtered, /Filtered by gpt-5\.4 \(Codex\)/);
  assert.match(filtered, /data-clear-model="1"[^>]*>Clear<\/button>/);
  assert.match(filtered, /1 of 7 days recorded/);
  assert.match(filtered, /2026-11-20: 1,000,000 input, 100,000 output, 1,100,000 total, ~\$4(?![\d.])/);
  assert.match(filtered, /2026-11-21: not recorded/);
  assert.match(filtered, /data-usage-model="gpt-5\.4"[^>]*aria-pressed="true"/);
  assert.match(filtered, /data-usage-model="o3-mini"[^>]*aria-pressed="false"/);
  assert.match(filtered, /Est\. cost \(USD\)<\/dt><dd>\$4\.77\*/);

  // When selected model belongs to a revoked or uninstalled agent, chart gracefully resets to unfiltered state
  const missingSelection = renderUsage([claude], 'total', 7, true, new Set(['claude']), now, 'en', { agentId: 'codex', model: 'gpt-5.4' });
  assert.doesNotMatch(missingSelection, /Filtered by|data-clear-model/);
  assert.match(missingSelection, /2026-11-21: 31 input, 9 output/);
});

test('source-reported OpenCode statistics stay in its own view, outside daily totals', () => {
  const opencode: Agent = {...claude, id:'opencode', name:'OpenCode', connected:true,
    usageHistory:[], modelHistory:[], usage:{
      inputTokens:100, outputTokens:20, cachedTokens:3, totalTokens:123, spendUsd:50,
      source:'Synthetic native session statistics', scope:'device', asOf:'2026-11-21T12:00:00Z',
      periodStart:'2026-11-01T00:00:00Z', periodEnd:'2026-11-21T12:00:00Z',
    },
  };
  opencode.reportedPeriodUsage = opencode.usage;
  const detail = renderUsage([opencode], 'opencode', 7, true, new Set(['opencode']), now);
  assert.match(detail, /Reported period totals/);
  assert.match(detail, /2026-11-01T00:00:00Z/);
  assert.match(detail, /2026-11-21T12:00:00Z/);
  assert.match(detail, /Input tokens<\/dt><dd>100/);
  assert.doesNotMatch(detail, /50\.00|data-period=/);
  assert.doesNotMatch(renderUsage([opencode], 'total', 30, true, new Set(['opencode']), now), /Synthetic native session statistics|<dd>100/);
  assert.doesNotMatch(renderUsage([opencode], 'opencode', 7, true, new Set(), now), /Reported period totals|<dd>100/);
  const offline: Agent = {...opencode, connected:false, historyCached:true, reportedPeriodUsage:null, savedPeriodUsage:opencode.usage};
  const saved = renderUsage([offline], 'opencode', 7, true, new Set(['opencode']), now);
  assert.match(saved, /Saved period totals/);
  assert.doesNotMatch(saved, /Reported period totals/);
});

test('cached or sign-in-required OpenCode usage is not shown as reported period totals', () => {
  const usage = {
    inputTokens:100, outputTokens:20, cachedTokens:3, totalTokens:123, spendUsd:null,
    source:'Unverified cached record', scope:'device' as const, asOf:'2026-11-21T12:00:00Z',
    periodStart:'2026-11-01T00:00:00Z', periodEnd:'2026-11-21T12:00:00Z',
  };
  const tool: Agent = {...claude, id:'opencode', name:'OpenCode', connected:true, historyCached:false, usage, usageHistory:[], modelHistory:[]};
  const approve = new Set(['opencode']);
  assert.doesNotMatch(renderUsage([tool], 'opencode', 7, true, approve, now), /Reported period totals|Unverified cached record/);
  const needsLogin = {...tool, authentication:'required', reportedPeriodUsage:usage} as const;
  assert.doesNotMatch(renderUsage([needsLogin], 'opencode', 7, true, approve, now), /Reported period totals|Unverified cached record/);
});
