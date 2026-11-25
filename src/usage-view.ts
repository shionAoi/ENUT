import { tr, type Lang } from './i18n.ts';
import { addCost, emptyCost, estimateModelDayUsd, formatEstimatedUsd, formatEstimatedUsdPlain } from './pricing.ts';
import type { Agent } from './types.ts';
import { formatNumber } from './logic.ts';
import { escapeHtml as e } from './view.ts';
import { presentAgent, usageTools } from './presentation.ts';
import { connectionAction } from './connection-actions.ts';
import {
  formatCacheMetric,
  localUsageAgentIds,
  renderTotalUsage,
  usageDates,
  usagePeriodButtons,
  type ModelFilter,
  type UsagePeriod,
} from './total-usage.ts';

export function selectedTool(agents: Agent[], id: string | null): Agent | null {
  const installed = agents.filter(a => a.installed).map(presentAgent);
  return installed.find(a => a.id === id) ?? installed[0] ?? null;
}

export function toolButtons(agents: Agent[], id: string | null, section: 'usage' | 'context', lang: Lang = 'en'): string {
  agents = section === 'usage' ? usageTools(agents) : agents.map(presentAgent);
  const total = section === 'usage' ? `<button type="button" data-tool="total" data-section="usage" aria-pressed="${id === 'total'}">${tr(lang, 'totalUsageBtn')}</button>` : '';
  const groupLabel = section === 'usage' ? tr(lang, 'usageToolLabel') : tr(lang, 'contextToolLabel');
  return `<div class="tool-select" role="group" aria-label="${groupLabel}">${total}${agents.filter(a => a.installed).map(a => `<button type="button" data-tool="${a.id}" data-section="${section}" aria-pressed="${a.id === id}">${e(a.name)}</button>`).join('')}</div>`;
}

export function accessButton(agent: Agent, desktop: boolean, approved: Set<string>, lang: Lang = 'en'): string {
  agent = presentAgent(agent);
  const action = connectionAction(agent, approved, lang);
  return `<button type="button" data-agent="${agent.id}" data-action="${action.action}" ${desktop ? '' : 'disabled'}>${action.label} ${e(agent.name)}</button>${approved.has(agent.id) && action.action !== 'revoke' ? `<button type="button" data-agent="${agent.id}" data-action="revoke" ${desktop ? '' : 'disabled'}>${tr(lang, 'revokeLabel')} ${e(agent.name)}</button>` : ''}`;
}

const validTokens = (n: number) => Number.isSafeInteger(n) && n >= 0;
const number = new Intl.NumberFormat();
const splitSum = (sum: bigint | null, value: number | undefined) =>
  sum !== null && value !== undefined && validTokens(value) ? sum + BigInt(value) : null;

function hasDatedLocalHistory(agent: Agent): boolean {
  return localUsageAgentIds.has(agent.id) && (
    (Array.isArray(agent.usageHistory) && agent.usageHistory.length > 0)
    || (Array.isArray(agent.modelHistory) && agent.modelHistory.length > 0)
  );
}

function reportedPeriod(agent: Agent, lang: Lang): string {
  if (hasDatedLocalHistory(agent)) return '';
  const fresh = agent.connected ? agent.reportedPeriodUsage : null;
  const u = fresh ?? agent.savedPeriodUsage;
  if (agent.id !== 'opencode' || agent.authentication === 'required' || !u || u.scope !== 'device' || !u.periodStart || !u.periodEnd) return '';
  const unavailable = tr(lang, 'unavailable');
  const metrics = [
    [tr(lang, 'inputTokens'), u.inputTokens],
    [tr(lang, 'outputTokens'), u.outputTokens],
    [tr(lang, 'cacheTokens'), u.cachedTokens],
    [tr(lang, 'totalTokens'), u.totalTokens],
  ] as const;
  return `<section class="usage-section" aria-labelledby="reported-usage-title"><h3 id="reported-usage-title">${fresh ? tr(lang, 'reportedPeriodTotals') : tr(lang, 'savedPeriodTotals')}</h3>
    <p class="meta">${e(u.periodStart)} → ${e(u.periodEnd)}</p><p class="meta">${tr(lang, 'openCodeSourceNote', { source: e(u.source) })}</p>
    <dl class="total-metrics">${metrics.map(([label, value]) => `<div><dt>${label}</dt><dd>${validTokens(value) ? formatNumber(value) : unavailable}</dd></div>`).join('')}</dl></section>`;
}

function periodSummary(agent: Agent, allowedDates: Set<string>, lang: Lang): string {
  if (!localUsageAgentIds.has(agent.id)) return '';
  const rows = (agent.usageHistory ?? []).filter(row =>
    allowedDates.has(row.date) && validTokens(row.inputTokens) && validTokens(row.outputTokens)
    && validTokens(row.cachedTokens) && validTokens(row.totalTokens));
  if (!rows.length) return '';
  let input = 0n, output = 0n, cached = 0n, total = 0n;
  let cacheCreation: bigint | null = 0n, cacheRead: bigint | null = 0n;
  let responses = 0n, counted = 0;
  let cost = emptyCost();
  for (const row of rows) {
    input += BigInt(row.inputTokens);
    output += BigInt(row.outputTokens);
    cached += BigInt(row.cachedTokens);
    total += BigInt(row.totalTokens);
    cacheCreation = splitSum(cacheCreation, row.cacheCreationTokens);
    cacheRead = splitSum(cacheRead, row.cacheReadTokens);
    if (row.recordedResponses !== undefined && validTokens(row.recordedResponses)) {
      responses += BigInt(row.recordedResponses);
      counted++;
    }
  }
  for (const row of agent.modelHistory ?? []) {
    if (!allowedDates.has(row.date) || !validTokens(row.totalTokens) || !row.model.trim()) continue;
    cost = addCost(cost, estimateModelDayUsd(row));
  }
  const metrics: [string, string][] = [
    [tr(lang, 'totalTokens'), number.format(total)],
    [tr(lang, 'inputTokens'), number.format(input)],
    [tr(lang, 'outputTokens'), number.format(output)],
    [tr(lang, 'cacheTokens'), formatCacheMetric(cached, cacheCreation, cacheRead, lang)],
    [tr(lang, 'estCostUsd'), formatEstimatedUsd(cost.usd, cost.partial, tr(lang, 'unavailable'))],
  ];
  if (counted > 0) metrics.push([tr(lang, 'recordedResponses'), number.format(responses)]);
  return `<dl class="total-metrics">${metrics.map(([label, value]) => `<div><dt>${label}</dt><dd>${value}</dd></div>`).join('')}</dl>`;
}

function history(agent: Agent, period: UsagePeriod, now: number, lang: Lang): string {
  const dates = usageDates(period, now);
  const allowedDates = new Set(dates);
  type ModelSum = {
    total: number;
    input: bigint | null;
    output: bigint | null;
    cached: bigint | null;
    cost: ReturnType<typeof emptyCost>;
  };
  const totals = new Map<string, ModelSum>();
  const rows = localUsageAgentIds.has(agent.id) ? agent.modelHistory : undefined;
  if (Array.isArray(rows) && rows.length <= 4096) {
    for (const row of rows) {
      if (!row || !allowedDates.has(row.date) || !validTokens(row.totalTokens) || typeof row.model !== 'string' || !row.model.trim()) continue;
      const prev = totals.get(row.model) ?? { total: 0, input: 0n, output: 0n, cached: 0n, cost: emptyCost() };
      totals.set(row.model, {
        total: prev.total + row.totalTokens,
        input: splitSum(prev.input, row.inputTokens),
        output: splitSum(prev.output, row.outputTokens),
        cached: splitSum(prev.cached, row.cachedTokens),
        cost: addCost(prev.cost, estimateModelDayUsd(row)),
      });
    }
  }
  const models = [...totals].filter(([, m]) => validTokens(m.total)).sort(([a], [b]) => a.localeCompare(b));
  const max = Math.max(1, ...models.map(([, m]) => m.total));
  const bars = models.map(([model, m]) => {
    const label = e(model), value = `${formatNumber(m.total)} ${tr(lang, 'tokenUnit')}`;
    const parts: string[] = [];
    if (m.input !== null) parts.push(`${tr(lang, 'inShort')} ${number.format(m.input)}`);
    if (m.output !== null) parts.push(`${tr(lang, 'outShort')} ${number.format(m.output)}`);
    if (m.cached !== null && m.cached > 0n) parts.push(`${tr(lang, 'cacheShort')} ${number.format(m.cached)}`);
    const usd = formatEstimatedUsdPlain(m.cost.usd, m.cost.partial);
    if (usd) parts.push(usd);
    const split = parts.length ? `<small class="model-split">${parts.join(' · ')}</small>` : '';
    return `<li><span>${label}${split}</span><strong>${value}</strong><progress value="${m.total}" max="${max}" aria-label="${label}" aria-valuetext="${value}"></progress></li>`;
  }).join('');
  const summary = periodSummary(agent, allowedDates, lang);
  const rangeMeta = period === 'all'
    ? tr(lang, 'allRetainedLabel')
    : `${tr(lang, 'lastDaysPrefix')} ${period} ${tr(lang, 'utcDaysSuffix')}`;
  return `<section class="usage-section" aria-labelledby="model-history-title"><h3 id="model-history-title">${tr(lang, 'tokensByModel')}</h3>${usagePeriodButtons(period, lang)}<p class="meta">${rangeMeta} · ${dates[0]} → ${dates.at(-1)}</p>${summary}${bars ? `${agent.connected ? '' : `<p role="status">${tr(lang, 'savedHistoryOffline')}</p>`}<ul class="source-totals">${bars}</ul>` : `<p class="unavailable">${tr(lang, 'noLocalModelUsagePeriod')}</p>`}</section>`;
}

export function renderUsage(
  agents: Agent[],
  id: string | null,
  period: UsagePeriod,
  desktop: boolean,
  approved: Set<string>,
  now = Date.now(),
  lang: Lang = 'en',
  selectedModel: ModelFilter = null,
): string {
  if (id === 'total') {
    const installed = usageTools(agents);
    const buttons = toolButtons(agents, id, 'usage', lang);
    return buttons + (desktop ? renderTotalUsage(installed, period, approved, now, lang, selectedModel) : `<p class="unavailable">${tr(lang, 'browserNoAccess')}</p>`);
  }
  const tools = usageTools(agents);
  const agent = tools.find(a => a.id === id) ?? tools[0];
  if (!agent) return `<p class="unavailable">${tr(lang, 'noInstalledToolsFull')}</p><p class="meta">${tr(lang, 'openDesktopDiscovery')}</p>`;
  const buttons = toolButtons(agents, agent.id, 'usage', lang);
  if (!desktop) return `${buttons}<p class="unavailable">${tr(lang, 'browserNoAccess')}</p>`;
  if (!approved.has(agent.id)) return `${buttons}<p class="unavailable">${tr(lang, 'permissionRequiredConnect')}</p>`;
  const reported = reportedPeriod(agent, lang);
  if (reported) return buttons + reported;
  if (!agent.connected && !agent.historyCached) return `${buttons}<p class="unavailable">${tr(lang, 'usageUnavailableDisconnected')}</p>`;
  return buttons + history(agent, period, now, lang);
}
