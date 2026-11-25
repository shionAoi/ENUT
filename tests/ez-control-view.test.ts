import test from 'node:test';
import assert from 'node:assert/strict';
import { formatCpuPercent, formatProcessMemory, renderEzControl } from '../src/ez-control-view.ts';
import type { ProcessControlSnapshot } from '../src/types.ts';

test('formatProcessMemory and formatCpuPercent match expected display scales', () => {
  assert.equal(formatProcessMemory(0), '0MB');
  assert.equal(formatProcessMemory(512 * 1024), '512KB');
  assert.equal(formatProcessMemory(3 * 1048576), '3MB');
  assert.equal(formatProcessMemory(5 * 1073741824), '5.0GB');
  assert.equal(formatCpuPercent(0), '0%');
  assert.equal(formatCpuPercent(0.4), '0.4%');
  assert.equal(formatCpuPercent(6.2), '6%');
});

test('renderEzControl shows RAM/CPU gauges, renames Antigravity to Gemini, toggles collapse, escapes tasks, and masks revoked agents', () => {
  const snapshot: ProcessControlSnapshot = {
    totalMemoryBytes: 5 * 1073741824,
    systemMemoryBytes: 15 * 1073741824,
    totalCpuPercent: 3.0,
    sampledAt: '2026-11-24T12:00:00Z',
    agents: [
      {
        agentId: 'codex',
        name: 'Codex',
        memoryBytes: 6 * 1048576,
        cpuPercent: 6.0,
        processes: [
          { pid: 1234, task: 'rust -tauri <bad>', memoryBytes: 3 * 1048576, cpuPercent: 3.0 },
          { pid: 56789, task: 'git clone...', memoryBytes: 2 * 1048576, cpuPercent: 2.0 },
        ],
      },
      {
        agentId: 'antigravity',
        name: 'Antigravity',
        memoryBytes: 5 * 1073741824,
        cpuPercent: 3.0,
        processes: [{ pid: 999, task: 'subagent...', memoryBytes: 5 * 1073741824, cpuPercent: 3.0 }],
      },
    ],
  };

  const approved = new Set(['codex', 'antigravity']);
  const expandedHtml = renderEzControl(snapshot, approved, new Set(), true, 'en');
  assert.match(expandedHtml, /RAM USAGE/);
  assert.match(expandedHtml, /CPU USAGE/);
  assert.match(expandedHtml, />5\.0<\/strong><span>GB<\/span>/);
  assert.match(expandedHtml, />9%<\/strong>/);
  assert.match(expandedHtml, />Codex</);
  assert.match(expandedHtml, />Gemini</);
  assert.doesNotMatch(expandedHtml, />Antigravity</);
  assert.match(expandedHtml, /rust -tauri &lt;bad&gt;/);
  assert.match(expandedHtml, />1234</);

  // Collapsed hides codex subtasks while keeping parent aggregate row
  const collapsedHtml = renderEzControl(snapshot, approved, new Set(['codex']), true, 'en');
  assert.match(collapsedHtml, /data-control-agent="codex"[^>]*aria-expanded="false"/);
  assert.doesNotMatch(collapsedHtml, />1234</);
  assert.match(collapsedHtml, />999</);

  // Revoking antigravity immediately removes it from gauges and table
  const revokedHtml = renderEzControl(snapshot, new Set(['codex']), new Set(), true, 'en');
  assert.doesNotMatch(revokedHtml, /Gemini|subagent|\b999\b/);
  assert.match(revokedHtml, />0\.0<\/strong><span>GB<\/span>/);
  assert.match(revokedHtml, />6%<\/strong>/);
});

test('renderEzControl shows safe guidance in browser, unapproved, and idle desktop states', () => {
  assert.match(
    renderEzControl(null, new Set(), new Set(), false, 'en'),
    /Open the desktop app to monitor live AI agent RAM, CPU, and subtasks/,
  );
  assert.match(
    renderEzControl(null, new Set(), new Set(), true, 'en'),
    /Connect installed tools in All tools/,
  );
  assert.match(
    renderEzControl(null, new Set(['codex']), new Set(), true, 'en'),
    /No active processes detected for connected AI tools/,
  );
});
