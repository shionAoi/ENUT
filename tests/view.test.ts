import { test } from 'node:test';
import assert from 'node:assert/strict';
import { renderAgents, renderSummary, escapeHtml } from '../src/view.ts';
import type { Agent } from '../src/types.ts';

const local: Agent = { id: 'ollama', name: 'Ollama', kind: 'local', installed: true, running: false,
  connected: true, status: 'unavailable', message: 'Runtime stopped', models: [{ id: 'llama', name: 'llama <3>', loaded: false, sizeBytes: 1024 ** 3 }], usage: null, quotas: [], updatedAt: null, capabilities: [] };
test('browser empty state contains no fictional agents and explains desktop connection', () => {
  const html = renderAgents([], 'usage', false);
  assert.match(html, /Desktop connection required/);
  assert.doesNotMatch(html, /Codex|Claude|Ollama/);
  assert.match(renderSummary([]), /Connect organization reporting separately/);
  assert.doesNotMatch(renderSummary([]), /\$0|Today|today/);
});
test('stopped runtime and reported inventory remain visible and escaped', () => {
  const html = renderAgents([local], 'models', true);
  assert.match(html, /Stopped/);
  assert.match(html, /llama &lt;3&gt;/);
  assert.match(html, /Not loaded/);
  assert.match(html, />1 GB</);
  assert.equal(escapeHtml('<script>"&'), '&lt;script&gt;&quot;&amp;');
});
test('disconnected cards cannot show stale usage, quotas or models', () => {
  for (const view of ['usage', 'quotas', 'models'] as const) {
    const html = renderAgents([{ ...local, connected: false }], view, true);
    assert.doesNotMatch(html, /llama &lt;3&gt;/);
    assert.match(html, /Connect/);
  }
});
test('approved but unavailable integration still exposes revocation', () => {
  const html = renderAgents([{ ...local, connected: false }], 'models', true, new Set(['ollama']));
  assert.match(html, /Revoke access/);
  assert.match(html, /Unavailable/);
  assert.match(html, /llama &lt;3&gt;/);
});
test('quota rendering shows exact real zero and reset source, without fake remaining tokens', () => {
  const html = renderAgents([{ ...local, quotas: [{ id: 'q', label: 'Weekly', remainingPercent: 0, resetsAt: '2026-11-08T12:00:00Z', source: 'Provider status' }] }], 'quotas', true);
  assert.match(html, /0% remaining/);
  assert.match(html, /Provider status/);
  assert.match(html, /value="0"/);
});
