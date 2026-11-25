import { test } from 'node:test';
import assert from 'node:assert/strict';
import { resolveTheme, saveTheme, selectAgents, summarize, countdown, percent, formatNumber, pollingDelay } from '../src/logic.ts';
import type { Agent } from '../src/types.ts';

const agent = (overrides: Partial<Agent> = {}): Agent => ({
  id: 'codex', name: 'Codex', kind: 'cloud', installed: true, running: null,
  connected: true, status: 'ready', message: '', models: [], quotas: [],
  usage: null, updatedAt: null, capabilities: [], ...overrides,
});
const usage = { inputTokens: 10, outputTokens: 20, cachedTokens: 0, totalTokens: 30,
  spendUsd: 1.25, source: 'report', scope: 'device' as const, asOf: '2026-11-07T12:00:00Z' };

test('theme follows OS until a valid explicit choice; unavailable storage is safe', () => {
  assert.equal(resolveTheme(null, true), 'dark');
  assert.equal(resolveTheme(null, false), 'light');
  assert.equal(resolveTheme('light', true), 'light');
  assert.equal(resolveTheme('dark', false), 'dark');
  assert.equal(resolveTheme('invalid', true), 'dark');
  assert.doesNotThrow(() => saveTheme({ setItem() { throw Error('denied'); } }, 'dark'));
});
test('detected-only filtering retains stopped installed runtimes', () => {
  const stopped = agent({ id: 'ollama', kind: 'local', running: false });
  const absent = agent({ installed: false });
  assert.deepEqual(selectAgents([stopped, absent], 'all'), [stopped]);
  assert.deepEqual(selectAgents([stopped], 'cloud'), []);
  assert.deepEqual(selectAgents([stopped], 'local'), [stopped]);
});
test('missing usage is unknown, actual zero remains zero, revoked data is excluded', () => {
  assert.equal(summarize([agent()]).tokens, null);
  assert.equal(summarize([agent({ usage: { ...usage, totalTokens: 0 } })]).tokens, 0);
  assert.equal(summarize([agent({ connected: false, usage })]).tokens, null);
  assert.equal(formatNumber(null), 'Unknown');
  assert.equal(formatNumber(0), '0');
});
test('device tokens and account spend stay separate and duplicate reports are not added', () => {
  const account = { ...usage, scope: 'account' as const, totalTokens: 900 };
  const sum = summarize([agent({ usage }), agent({ id: 'claude', usage: account }), agent({ id: 'gemini', usage: account })]);
  assert.equal(sum.tokens, 30);
  assert.equal(sum.spend, 1.25);
  assert.equal(sum.accountTokens, 900);
  assert.equal(sum.tokenSources, 1);
});
test('device spend is not account reporting and malformed token counts remain unknown', () => {
  assert.equal(summarize([agent({ usage })]).spend, null);
  assert.equal(summarize([agent({ usage: { ...usage, totalTokens: NaN } })]).tokens, null);
});
test('nearest known future reset and safe countdown boundaries', () => {
  const now = Date.parse('2026-11-07T12:00:00Z');
  const quotas = ['2026-11-07T14:00:00Z', 'invalid', '2026-11-07T13:00:00Z'].map((resetsAt, i) => ({ id: String(i), label: 'Window', remainingPercent: null, resetsAt, source: 'provider' }));
  assert.equal(summarize([agent({ quotas })], now).reset, '2026-11-07T13:00:00Z');
  assert.equal(countdown(null, now), 'Unavailable');
  assert.equal(countdown('invalid', now), 'Unavailable');
  assert.equal(countdown('2026-11-07T11:00:00Z', now), 'Reset pending');
  assert.equal(countdown('2026-11-07T13:00:00Z', now), '1h 0m');
  assert.equal(countdown('2026-11-07T12:00:01Z', now), '<1m');
});
test('quota values cannot create invented percentages', () => {
  assert.equal(percent(null), null);
  assert.equal(percent(-1), null);
  assert.equal(percent(101), null);
  assert.equal(percent(NaN), null);
  assert.equal(percent(0), 0);
  assert.equal(percent(100), 100);
});
test('polling backs off conservatively after failures', () => {
  assert.equal(pollingDelay(0), 45000);
  assert.equal(pollingDelay(1), 90000);
  assert.equal(pollingDelay(9), 360000);
});
