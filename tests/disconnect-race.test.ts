import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Monitor } from '../src/controller.ts';
import type { Snapshot } from '../src/types.ts';

test('disconnect intent masks billing in older permission snapshots', async () => {
  const old: Snapshot = { agents: [], scannedAt:'2026-11-07T00:00:00Z', platform:'test', warnings:[],
    billing:[{ provider:'openai', spendUsd:12, periodStart:'2026-11-01T00:00:00Z', periodEnd:'2026-11-07T00:00:00Z', asOf:'2026-11-07T00:00:00Z', message:'Synthetic organization report' }] };
  let finishPermission!: (value: Snapshot) => void;
  let disconnectRequested = false;
  const published: number[] = [];
  const monitor = new Monitor(true, command => {
    if (command === 'set_permission') return new Promise(resolve => { finishPermission = resolve; });
    assert.equal(command, 'disconnect_billing');
    return Promise.resolve({...old,billing:[]});
  }, () => { if (disconnectRequested) published.push(monitor.snapshot.billing?.length ?? 0); });
  monitor.snapshot = old;
  const permission = monitor.permission('ollama', true);
  await Promise.resolve();
  disconnectRequested = true;
  const disconnect = monitor.billing('openai');
  assert.deepEqual(monitor.snapshot.billing, []);
  finishPermission(old);
  await permission;
  await disconnect;
  assert.ok(published.length > 0);
  assert.ok(published.every(count => count === 0), `Disconnected reports reappeared: ${JSON.stringify(published)}`);
});
