import test from 'node:test';
import assert from 'node:assert/strict';
import { resolveLang, saveLang, tr, type Lang } from '../src/i18n.ts';
import { renderShell } from '../src/shell.ts';
import { renderBilling, renderTools } from '../src/view.ts';
import { renderUsage } from '../src/usage-view.ts';
import { renderContext } from '../src/context-view.ts';
import { renderEzControl } from '../src/ez-control-view.ts';
import { Monitor } from '../src/controller.ts';
import type { Agent, BillingReport } from '../src/types.ts';

test('resolves and saves ES/EN language preference safely', () => {
  assert.equal(resolveLang(null), 'en');
  assert.equal(resolveLang('fr'), 'en');
  assert.equal(resolveLang('es'), 'es');
  const store = new Map<string, string>();
  saveLang({ setItem: (k, v) => { store.set(k, v); } }, 'es');
  assert.equal(store.get('ai-monitor-lang'), 'es');
  saveLang({ setItem: () => { throw new Error('denied'); } }, 'es');
});

test('Settings includes ES/EN language toggle and Spanish mode translates all tabs in neutral Spanish', () => {
  const enShell = renderShell('en');
  assert.match(enShell, /id="language"[^>]*data-lang="en"[^>]*>EN → ES<\/button>/);
  const esShell = renderShell('es');
  assert.match(esShell, /id="language"[^>]*data-lang="es"[^>]*>ES → EN<\/button>/);
  assert.match(esShell, />Herramientas<\/span>/);
  assert.match(esShell, />Uso</);
  assert.match(esShell, />Contexto</);
  assert.match(esShell, />Configuración</);
  assert.match(esShell, />Idioma</);

  const now = Date.parse('2026-11-23T12:00:00Z');
  const codex: Agent = {
    id: 'codex',
    name: 'Codex',
    kind: 'cloud',
    installed: true,
    running: true,
    connected: true,
    status: 'ready',
    message: '',
    models: [],
    usage: null,
    quotas: [{ id: 'w', label: 'Weekly', remainingPercent: 80, resetsAt: '2026-11-24T12:00:00Z', source: 'Provider' }],
    updatedAt: '2026-11-23T12:00:00Z',
    capabilities: ['quota', 'localUsage', 'history', 'context'],
    usageHistory: [{ date: '2026-11-23', inputTokens: 1000, outputTokens: 500, cachedTokens: 200, cacheCreationTokens: 0, cacheReadTokens: 200, totalTokens: 1700, recordedResponses: 2 }],
    modelHistory: [{ date: '2026-11-23', model: 'gpt-5.4', inputTokens: 1000, outputTokens: 500, cachedTokens: 200, cacheCreationTokens: 0, cacheReadTokens: 200, totalTokens: 1700 }],
    context: { mcps: [{ name: 'docs', status: 'connected' }], skills: ['skill-a'], note: '' },
  };
  const approved = new Set(['codex']);

  const esTools = renderTools([codex], 'quotas', true, approved, new Set(['codex']), now, 'es');
  assert.match(esTools, /En ejecución/);
  assert.match(esTools, /Detalles de cuota/);
  assert.match(esTools, /80% restante/);

  const esUsage = renderUsage([codex], 'total', 7, true, approved, now, 'es');
  assert.match(esUsage, /USO TOTAL/);
  assert.match(esUsage, /Uso de agentes/);
  assert.match(esUsage, /Tokens totales/);
  assert.match(esUsage, /Costo est\. \(USD\)/);
  assert.match(esUsage, /Tokens por día/);
  assert.match(esUsage, /Est\. USD/);

  const esContext = renderContext([codex], 'codex', true, approved, 'es');
  assert.match(esContext, /Habilidades instaladas/);
  assert.match(esContext, /Conectado/);

  // Neutral professional Spanish (no voseo / Rioplatense slang)
  const combinedEs = `${esShell}${esTools}${esUsage}${esContext}${tr('es', 'clickModelHint')}`;
  assert.doesNotMatch(combinedEs, /\b(?:hacé|elegí|mirá|tenés|querés|podés|vos|che|dale)\b/i);
});

test('Spanish UI surfaces do not leak English labels in tools, billing, notices, or auth copy', async () => {
  const agent: Agent = {
    id: 'codex',
    name: 'Codex',
    kind: 'cloud',
    installed: true,
    running: true,
    connected: true,
    status: 'ready',
    message: '',
    models: [],
    usage: null,
    quotas: [{ id: 'q1', label: 'Session', remainingPercent: 50, resetsAt: '2026-11-23T15:00:00Z', source: 'Codex' }],
    updatedAt: '2026-11-23T11:59:00Z',
    capabilities: ['quota'],
  };
  const toolsEs = renderTools([agent], 'quotas', true, new Set(['codex']), new Set(['codex']), Date.parse('2026-11-23T12:00:00Z'), 'es');
  assert.doesNotMatch(toolsEs, /aria-label="About | for |remaining allowance/);
  assert.match(toolsEs, /aria-label="Acerca de Revocar para Codex"/);
  assert.match(toolsEs, /aria-label="Detalles de cuota para Codex"/);
  assert.match(toolsEs, /aria-label="Session límite restante"/);

  const pendingReport: BillingReport = {
    provider: 'openai',
    spendUsd: null,
    periodStart: '',
    periodEnd: '',
    asOf: '2026-11-23T11:59:00Z',
    message: 'En espera',
  };
  const billingEs = renderBilling([agent], [pendingReport], true, 'es');
  assert.doesNotMatch(billingEs, /\bUnavailable\b/);
  assert.match(billingEs, /No disponible/);

  const defaultMonitor = new Monitor(false, async () => ({ agents: [], scannedAt: '', platform: 'linux', warnings: [] }), () => {});
  assert.equal(defaultMonitor.notice, 'Desktop connection required');

  let currentLang: Lang = 'en';
  const langMonitor = new Monitor(false, async () => ({ agents: [], scannedAt: '', platform: 'linux', warnings: [] }), () => {}, () => currentLang);
  assert.equal(langMonitor.notice, 'Desktop connection required');
  currentLang = 'es';
  assert.equal(langMonitor.notice, 'Se requiere conexión de escritorio');

  const failingMonitor = new Monitor(true, async () => { throw new Error('boom'); }, () => {}, () => currentLang);
  await failingMonitor.refresh();
  assert.doesNotMatch(failingMonitor.notice, /Could not update desktop data/);
  assert.match(failingMonitor.notice, /No se pudieron actualizar/);

  const esStrings = [
    tr('es', 'accessNotGranted'),
    tr('es', 'noticeUpdateFailed'),
    tr('es', 'noticeReportingDisconnectFailed'),
    tr('es', 'noticeReportingConnectFailed'),
    tr('es', 'noticePermissionGrantFailed'),
    tr('es', 'noticeRevokeFailed'),
    tr('es', 'authTitle'),
    tr('es', 'authLoginTo', { tool: 'Codex' }),
    tr('es', 'authIntro1'),
    tr('es', 'authIntro2'),
    tr('es', 'authPasteHeading'),
    tr('es', 'authPasteBody'),
    tr('es', 'authDeviceCodePrompt', { code: '1234' }),
    tr('es', 'authFinishedNotice'),
    tr('es', 'authPollError'),
    tr('es', 'authCancelError'),
    tr('es', 'authCodeSent'),
    tr('es', 'authCodeSubmitError'),
  ].join(' ');
  assert.doesNotMatch(esStrings, /\b(?:hacé|elegí|mirá|tenés|querés|podés|vos|che|dale)\b/i);

  const esControl = renderEzControl(
    {
      totalMemoryBytes: 5 * 1073741824,
      systemMemoryBytes: 16 * 1073741824,
      totalCpuPercent: 4,
      sampledAt: '2026-11-24T12:00:00Z',
      agents: [{ agentId: 'codex', name: 'Codex', memoryBytes: 5 * 1073741824, cpuPercent: 4, processes: [{ pid: 42, task: 'git clone', memoryBytes: 5 * 1073741824, cpuPercent: 4 }] }],
    },
    new Set(['codex']),
    new Set(),
    true,
    'es',
  );
  assert.match(esControl, /USO DE RAM/);
  assert.match(esControl, /USO DE CPU/);
  assert.match(esControl, />TAREA</);
  assert.match(esControl, />Memoria</);
  assert.doesNotMatch(esControl, /RAM USAGE|CPU USAGE|>TASK<|>Memory</);

  const esEmptyControl = renderEzControl(null, new Set(), new Set(), false, 'es')
    + renderEzControl(null, new Set(), new Set(), true, 'es')
    + renderEzControl(null, new Set(['codex']), new Set(), true, 'es');
  assert.doesNotMatch(esEmptyControl, /\b(?:hacé|elegí|mirá|tenés|querés|podés|vos|che|dale|Open|Connect|No active)\b/i);
});
