import test from 'node:test';
import assert from 'node:assert/strict';
import { Monitor } from '../src/controller.ts';
import type { Snapshot } from '../src/types.ts';

const empty: Snapshot = { agents: [], scannedAt: '', platform: 'test', warnings: [], billing: [] };
const report: Snapshot = { ...empty, billing: [{ provider: 'openai', spendUsd: 12, periodStart: '', periodEnd: '', asOf: null, message: 'Synthetic report' }] };

test('revoke and reporting disconnect reach the backend before a pending refresh completes', async () => {
  let finish!: (snapshot: Snapshot) => void;
  const commands: string[] = [];
  const monitor = new Monitor(true, command => {
    commands.push(command);
    return command === 'refresh_usage' ? new Promise(resolve => { finish = resolve; }) : Promise.resolve(empty);
  }, () => {});
  monitor.approved.add('codex');
  monitor.snapshot = report;
  const refresh = monitor.refresh();
  const revoke = monitor.permission('codex', false);
  const disconnect = monitor.billing('openai');
  await new Promise(resolve => setImmediate(resolve));
  try {
    assert.deepEqual(commands, ['refresh_usage', 'set_permission', 'disconnect_billing']);
    assert.equal(monitor.approved.has('codex'), false);
    assert.deepEqual(monitor.snapshot.billing, []);
    assert.equal(monitor.busy, true, 'The earlier refresh is still pending');
  } finally {
    finish(report);
    await Promise.all([refresh, revoke, disconnect]);
  }
  assert.deepEqual(monitor.snapshot.billing, []);
  assert.equal(monitor.busy, false);
});

test('an unrelated permission does not hide a successful reporting reconnect', async () => {
  let finish!: (snapshot: Snapshot) => void;
  const monitor = new Monitor(true, command => command === 'connect_billing'
    ? new Promise(resolve => { finish = resolve; }) : Promise.resolve(report), () => {});
  await monitor.billing('openai');
  const reconnect = monitor.billing('openai', 'synthetic-key-not-a-secret');
  await new Promise(resolve => setImmediate(resolve));
  const permission = monitor.permission('ollama', true);
  finish(report);
  await Promise.all([reconnect, permission]);
  await monitor.refresh();
  assert.deepEqual(monitor.snapshot.billing, report.billing);
  assert.equal(monitor.approved.has('ollama'), true);
});

test('a queued disconnect wins over an older reporting reconnect', async () => {
  let finish!: (snapshot: Snapshot) => void;
  const monitor = new Monitor(true, command => command === 'connect_billing'
    ? new Promise(resolve => { finish = resolve; }) : Promise.resolve(report), () => {});
  const reconnect = monitor.billing('openai', 'synthetic-key-not-a-secret');
  await new Promise(resolve => setImmediate(resolve));
  const disconnect = monitor.billing('openai');
  finish(report);
  await Promise.all([reconnect, disconnect]);
  await monitor.refresh();
  assert.deepEqual(monitor.snapshot.billing, []);
});

test('a rejected reporting credential keeps its error instead of starting a refresh', async () => {
  const commands: string[] = [];
  const monitor = new Monitor(true, command => {
    commands.push(command);
    if (command === 'connect_billing') return Promise.reject(Error('Invalid synthetic credential'));
    return Promise.resolve(empty);
  }, () => {});
  await monitor.billing('openai', 'invalid');
  assert.deepEqual(commands, ['connect_billing']);
  assert.match(monitor.notice, /Reporting could not be connected/);
  assert.equal(monitor.busy, false);
});
