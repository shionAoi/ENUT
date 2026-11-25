import test from 'node:test';
import assert from 'node:assert/strict';
import { renderBilling, renderSummary } from '../src/view.ts';
import { billingTotal } from '../src/logic.ts';
import type { BillingReport, Agent } from '../src/types.ts';

const report: BillingReport = { provider: 'openai', spendUsd: 12.5, periodStart: '2026-11-01T00:00:00Z', periodEnd: '2026-11-07T17:00:00Z', asOf: '2026-11-07T17:00:00Z', message: 'Organization-wide, delayed' };
const agent: Agent = { id:'codex', name:'Codex', kind:'cloud', installed:true, running:null, connected:false, status:'detected', message:'', models:[], usage:null, quotas:[], updatedAt:null, capabilities:[] };

test('organization cost reports deduplicate providers and remain separate from tokens', () => {
  assert.equal(billingTotal([report, report]), 12.5);
  assert.equal(billingTotal([{ ...report, spendUsd:null }]), null);
  assert.match(renderSummary([], [report]), /\$12\.5</);
  assert.match(renderSummary([], [report]), /month-to-date/);
});

test('billing connection controls exist only for installed matching cloud agents', () => {
  assert.equal(renderBilling([], [report], true), '');
  assert.match(renderBilling([agent], [], true), /Connect OpenAI reporting/);
  assert.match(renderBilling([agent], [report], true), /Disconnect/);
  assert.doesNotMatch(renderBilling([{...agent,installed:false}], [], true), /Connect/);
  assert.doesNotMatch(renderBilling([agent], [], false), /Connect OpenAI reporting/);
});
