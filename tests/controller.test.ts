import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Monitor } from '../src/controller.ts';
import type { Agent, Snapshot } from '../src/types.ts';

const snapshot: Snapshot = { agents: [], scannedAt: '2026-11-07T12:00:00Z', platform: 'test', warnings: [] };
test('session permission is distinct from a runtime connection succeeding', async () => {
  const monitor = new Monitor(true, async () => snapshot, () => {});
  await monitor.permission('ollama', true);
  assert.equal(monitor.approved.has('ollama'), true);
  await monitor.permission('ollama', false);
  assert.equal(monitor.approved.has('ollama'), false);
});
test('installation removal forgets the UI session approval', async () => {
  const monitor = new Monitor(true, async () => snapshot, () => {});
  monitor.approved.add('ollama');
  await monitor.refresh(true);
  assert.equal(monitor.approved.has('ollama'), false);
});
test('browser has no fake entries and never invokes desktop commands', async () => {
  const monitor = new Monitor(false, async () => { throw Error('unexpected invoke'); }, () => {});
  await monitor.refresh();
  assert.deepEqual(monitor.snapshot.agents, []);
  assert.equal(monitor.notice, 'Desktop connection required');
});
test('a queued permission prevents refresh from starting another IPC operation', async () => {
  let finish!: (s: Snapshot) => void;
  const commands: string[] = [];
  const monitor = new Monitor(true, command => {
    commands.push(command);
    if (command === 'set_permission') return new Promise(r => { finish = r; });
    return Promise.resolve(snapshot);
  }, () => {});
  const permission = monitor.permission('ollama', true);
  const refresh = monitor.refresh();
  await Promise.resolve();
  assert.deepEqual(commands, ['set_permission']);
  finish(snapshot);
  await permission;
  await refresh;
});
test('rapid allow followed by revoke never briefly restores approved state', async () => {
  let finish!: (s: Snapshot) => void;
  const calls: unknown[] = [];
  const states: boolean[] = [];
  const monitor = new Monitor(true, (_, args) => {
    calls.push(args?.allowed);
    return calls.length === 1 ? new Promise(r => { finish = r; }) : Promise.resolve(snapshot);
  }, () => { states.push(monitor.approved.has('ollama')); });
  const grant = monitor.permission('ollama', true);
  await Promise.resolve();
  const revoke = monitor.permission('ollama', false);
  try {
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.deepEqual([...calls], [true]);
  } finally {
    finish(snapshot);
    await Promise.all([grant, revoke]);
  }
  assert.deepEqual(calls, [true, false]);
  assert.equal(states.includes(true), false);
});
test('refresh does not overlap and failures preserve last successful snapshot', async () => {
  let finish!: (s: Snapshot) => void;
  let calls = 0;
  const monitor = new Monitor(true, () => { calls++; return new Promise(r => { finish = r; }); }, () => {});
  const first = monitor.refresh();
  await monitor.refresh();
  assert.equal(calls, 1);
  finish(snapshot);
  await first;
  assert.equal(monitor.snapshot.scannedAt, snapshot.scannedAt);
  const failing = new Monitor(true, async () => { throw Error('secret server details'); }, () => {});
  await failing.refresh();
  assert.equal(failing.failures, 1);
  assert.equal(failing.notice.includes('secret'), false);
  assert.equal(failing.busy, false);
});
test('revocation immediately hides data and invalidates an in-flight refresh', async () => {
  const populated: Snapshot = { ...snapshot, agents: [{ id: 'codex', name: 'Codex', kind: 'cloud', installed: true, running: true, connected: true, status: 'ready', message: 'private status', models: [], quotas: [], usage: { inputTokens: 1, outputTokens: 2, cachedTokens: 0, totalTokens: 3, spendUsd: null, scope: 'device', source: 'logs', asOf: snapshot.scannedAt }, updatedAt: snapshot.scannedAt, capabilities: [] }] };
  let finish!: (s: Snapshot) => void;
  const commands: string[] = [];
  const monitor = new Monitor(true, command => {
    commands.push(command);
    if (command === 'set_permission') return Promise.resolve({ ...populated, agents: populated.agents.map(a => ({ ...a, connected: false, usage: null, message: '', updatedAt: null })) });
    return new Promise(r => { finish = r; });
  }, () => {});
  monitor.snapshot = populated;
  const refresh = monitor.refresh();
  const revoke = monitor.permission('codex', false);
  assert.equal(monitor.snapshot.agents[0].usage, null);
  assert.equal(monitor.snapshot.agents[0].connected, false);
  finish(populated);
  await refresh;
  await revoke;
  assert.equal(monitor.snapshot.agents[0].usage, null);
  assert.deepEqual(commands, ['refresh_usage', 'set_permission']);
});

function agent(id: Agent['id'], message: string): Agent {
  return {
    id, name: id, kind: 'cloud', installed: true, running: true, connected: true,
    monitoringAllowed: true, status: 'ready', message, models: [], quotas: [], usage: null,
    context: { mcps: [], skills: [message], note: '' }, authentication: 'available',
    updatedAt: snapshot.scannedAt, capabilities: [],
  };
}

test('a second tool revokes at IPC while the first tool is still draining', async () => {
  const requests: unknown[] = [];
  let release!: (value: Snapshot) => void;
  const draining = new Promise<Snapshot>(resolve => { release = resolve; });
  const populated = { ...snapshot, agents: [agent('claude', 'old Claude'), agent('codex', 'old Codex')] };
  const monitor = new Monitor(true, async (command, args) => {
    assert.equal(command, 'set_permission');
    assert.equal(args?.allowed, false);
    requests.push(args?.agentId);
    return args?.agentId === 'claude' ? draining : populated;
  }, () => {});
  monitor.snapshot = populated;
  const first = monitor.permission('claude', false);
  await Promise.resolve();
  const second = monitor.permission('codex', false);
  try {
    assert.ok(monitor.snapshot.agents.every(a => a.context === null && !a.monitoringAllowed));
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.deepEqual([...requests], ['claude', 'codex']);
    assert.equal(monitor.busy, true);
  } finally {
    release(populated);
    await Promise.all([first, second]);
  }
  assert.ok(monitor.snapshot.agents.every(a => a.context === null && !a.monitoringAllowed));
  assert.equal(monitor.busy, false);
});

test('overlapping grants cannot publish stale rows, warnings, or billing from another lane', async () => {
  const initial: Snapshot = {
    ...snapshot, warnings: ['current warning'],
    billing: [{ provider: 'openai', spendUsd: 22, periodStart: '', periodEnd: '', asOf: null, message: 'Current report' }],
    agents: [agent('claude', 'Current Claude'), agent('codex', 'Current Codex')],
  };
  const oldCodex = { ...agent('codex', 'Old Codex'), monitoringAllowed: false };
  const freshCodex = agent('codex', 'New Codex');
  const codexReply = { ...snapshot, warnings: ['stale Codex warning'], billing: [], agents: [agent('claude', 'Old Claude'), freshCodex] };
  const claudeReply = { ...snapshot, warnings: ['stale Claude warning'], billing: [], agents: [agent('claude', 'New Claude'), oldCodex] };
  let release!: (value: Snapshot) => void;
  const draining = new Promise<Snapshot>(resolve => { release = resolve; });
  const monitor = new Monitor(true, async (_, args) => args?.agentId === 'claude' ? draining : codexReply, () => {});
  monitor.snapshot = initial;
  const claude = monitor.permission('claude', true);
  await Promise.resolve();
  const codex = monitor.permission('codex', true);
  try {
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.deepEqual(monitor.snapshot.agents, [initial.agents[0], freshCodex]);
    assert.equal(monitor.approved.has('codex'), true);
  } finally {
    release(claudeReply);
    await Promise.all([claude, codex]);
  }
  assert.deepEqual(monitor.snapshot.agents, [claudeReply.agents[0], freshCodex]);
  assert.equal(monitor.approved.has('codex'), true);
  assert.deepEqual(monitor.snapshot.warnings, initial.warnings);
  assert.deepEqual(monitor.snapshot.billing, initial.billing);
});

test('sign-in pause waits for its own tool, not an unrelated draining revoke', async () => {
  let releaseClaude!: (value: Snapshot) => void;
  let releaseCodex!: (value: Snapshot) => void;
  const claudeLatch = new Promise<Snapshot>(resolve => { releaseClaude = resolve; });
  const codexLatch = new Promise<Snapshot>(resolve => { releaseCodex = resolve; });
  const monitor = new Monitor(true, async (_, args) => args?.agentId === 'claude' ? claudeLatch : codexLatch, () => {});
  const claude = monitor.permission('claude', false);
  const codex = monitor.permission('codex', true);
  await Promise.resolve();
  let paused = false;
  const pause = monitor.pause('codex').then(() => { paused = true; });
  try {
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.equal(paused, false);
    releaseCodex({ ...snapshot, agents: [agent('codex', 'Superseded grant')] });
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.equal(paused, true);
    assert.equal(monitor.approved.has('codex'), false);
  } finally {
    releaseClaude(snapshot);
    releaseCodex(snapshot);
    await Promise.all([claude, codex, pause]);
  }
});
