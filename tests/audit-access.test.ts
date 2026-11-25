import test from 'node:test';
import assert from 'node:assert/strict';
import { renderContext } from '../src/context-view.ts';
import { renderUsage } from '../src/usage-view.ts';
import type { Agent } from '../src/types.ts';

test('Context keeps Revoke beside Connect or Login; Usage remains chart-only', () => {
  const tool: Agent = { id: 'opencode', name: 'OpenCode', kind: 'cloud', installed: true, running: null, connected: false, status: 'error', message: '', models: [], usage: null, quotas: [], updatedAt: null, capabilities: [] };
  for (const authentication of ['unknown', 'required'] as const) {
    const agent = { ...tool, authentication };
    for (const desktop of [true, false]) {
      const approved = new Set(['opencode']);
      const html=renderContext([agent], agent.id, desktop, approved);
      assert.match(html, new RegExp(`data-action="${authentication === 'required' ? 'login' : 'connect'}"`));
      assert.equal((html.match(/data-action="revoke"/g) ?? []).length, 1);
      assert.match(html, desktop ? /data-action="revoke"\s*>/ : /data-action="revoke"\s+disabled/);
      assert.doesNotMatch(renderUsage([agent], agent.id, 7, desktop, approved), /data-action=|<progress/);
    }
  }
});
