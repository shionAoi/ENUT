import { tr, type Lang } from './i18n.ts';
import {
  addCost,
  emptyCost,
  estimateModelDayUsd,
  formatEstimatedUsd,
  formatEstimatedUsdPlain,
  mergeCost,
  type CostAggregate,
} from './pricing.ts';
import type { Agent } from './types.ts';
import { escapeHtml as e } from './view.ts';

export type UsagePeriod = 7 | 30 | 90 | 'all';
export type ModelFilter = { agentId: string; model: string } | null;
export const usagePeriods = [7, 30, 90, 'all'] as const;
export const localUsageAgentIds = new Set(['claude', 'gemini', 'codex', 'opencode', 'antigravity', 'hermes']);
const number = new Intl.NumberFormat();
const short = new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 2 });
const valid = (n: number) => Number.isSafeInteger(n) && n >= 0;
const count = (n: bigint | null, unavailableLabel: string) =>
  n === null ? `<span aria-label="${unavailableLabel}">—</span>` : number.format(n);
const splitSum = (sum: bigint | null, value: number | undefined) =>
  sum !== null && value !== undefined && valid(value) ? sum + BigInt(value) : null;
type ModelCounts = {
  total: bigint;
  input: bigint | null;
  output: bigint | null;
  cached: bigint | null;
  cost: CostAggregate;
};
type DayCounts = { input: bigint; output: bigint; cached: bigint; total: bigint; cost: CostAggregate };

export function usageDates(period: UsagePeriod, now: number) {
  const end = Math.floor(now / 86400000);
  const length = period === 'all' ? 90 : period;
  return Array.from({ length }, (_, i) => new Date((end - length + 1 + i) * 86400000).toISOString().slice(0, 10));
}

export function usagePeriodButtons(period: UsagePeriod, lang: Lang = 'en') {
  return `<div class="period-select" role="group" aria-label="${tr(lang, 'historyPeriodLabel')}">${usagePeriods.map(days =>
    `<button type="button" data-period="${days}" aria-pressed="${period === days}">${days === 'all' ? tr(lang, 'allDaysBtn') : `${days} ${tr(lang, 'daysSuffix')}`}</button>`
  ).join('')}</div>`;
}

export function formatCacheMetric(
  cached: bigint | null,
  creation: bigint | null,
  read: bigint | null,
  lang: Lang = 'en',
) {
  if (cached === null) return count(null, tr(lang, 'unavailable'));
  const split = creation !== null && read !== null && cached > 0n
    ? `<small class="cache-split">${tr(lang, 'writePrefix')} ${short.format(Number(creation))} · ${tr(lang, 'readPrefix')} ${short.format(Number(read))}</small>`
    : '';
  return `${number.format(cached)}${split}`;
}

function table(id: string, columns: string[], rows: string) {
  return `<div class="table-scroll total-table" role="region" aria-labelledby="${id}" tabindex="0"><table><thead><tr>${columns.map(
    label => `<th scope="col">${label}</th>`
  ).join('')}</tr></thead><tbody>${rows}</tbody></table></div>`;
}

export function renderTotalUsage(
  agents: Agent[],
  period: UsagePeriod,
  approved: Set<string>,
  now: number,
  lang: Lang = 'en',
  selectedModel: ModelFilter = null,
) {
  const unavailableLabel = tr(lang, 'unavailable');
  const dates = usageDates(period, now);
  const allowedDates = new Set(dates);
  const daily = new Map<string, DayCounts>();
  const models: ({ name: string; agent: string; agentId: string } & ModelCounts)[] = [];
  const agentRows = [];
  let allInput = 0n, allOutput = 0n, allCached = 0n, allTotal = 0n;
  let allCacheCreation: bigint | null = 0n, allCacheRead: bigint | null = 0n;
  let allCost = emptyCost();
  let responses = 0n, countedRows = 0, observedRows = 0;

  for (const agent of agents) {
    const local = localUsageAgentIds.has(agent.id) && approved.has(agent.id) && (agent.connected || agent.historyCached);
    const rows = local
      ? (agent.usageHistory ?? []).filter(row =>
        allowedDates.has(row.date) && valid(row.inputTokens) && valid(row.outputTokens)
        && valid(row.cachedTokens) && valid(row.totalTokens))
      : [];
    let input = 0n, output = 0n, cached = 0n, total = 0n;
    for (const row of rows) {
      const day = daily.get(row.date) ?? { input: 0n, output: 0n, cached: 0n, total: 0n, cost: emptyCost() };
      day.input += BigInt(row.inputTokens);
      day.output += BigInt(row.outputTokens);
      day.cached += BigInt(row.cachedTokens);
      day.total += BigInt(row.totalTokens);
      daily.set(row.date, day);
      input += BigInt(row.inputTokens);
      output += BigInt(row.outputTokens);
      cached += BigInt(row.cachedTokens);
      total += BigInt(row.totalTokens);
      allCacheCreation = splitSum(allCacheCreation, row.cacheCreationTokens);
      allCacheRead = splitSum(allCacheRead, row.cacheReadTokens);
      observedRows++;
      if (row.recordedResponses !== undefined && valid(row.recordedResponses)) {
        responses += BigInt(row.recordedResponses);
        countedRows++;
      }
    }
    const totals = new Map<string, ModelCounts>();
    let agentCost = emptyCost();
    if (local) for (const row of agent.modelHistory ?? []) {
      if (!allowedDates.has(row.date) || !valid(row.totalTokens) || !row.model.trim()) continue;
      const est = estimateModelDayUsd(row);
      agentCost = addCost(agentCost, est);
      allCost = addCost(allCost, est);
      const day = daily.get(row.date);
      if (day) day.cost = addCost(day.cost, est);
      const previous = totals.get(row.model) ?? {
        total: 0n,
        input: 0n,
        output: 0n,
        cached: 0n,
        cost: emptyCost(),
      };
      totals.set(row.model, {
        total: previous.total + BigInt(row.totalTokens),
        input: splitSum(previous.input, row.inputTokens),
        output: splitSum(previous.output, row.outputTokens),
        cached: splitSum(previous.cached, row.cachedTokens),
        cost: addCost(previous.cost, est),
      });
    }
    for (const [name, values] of totals) {
      models.push({ name, agent: agent.name, agentId: agent.id, ...values });
    }
    allInput += input;
    allOutput += output;
    allCached += cached;
    allTotal += total;
    agentRows.push({
      agent,
      input: rows.length ? input : null,
      output: rows.length ? output : null,
      cached: rows.length ? cached : null,
      total: rows.length ? total : null,
      cost: rows.length ? agentCost : emptyCost(),
    });
  }

  const activeModel = selectedModel
    ? models.find(m => m.agentId === selectedModel.agentId && m.name === selectedModel.model) ?? null
    : null;

  const chartDaily = new Map<string, DayCounts>();
  if (activeModel) {
    const targetAgent = agents.find(a => a.id === activeModel.agentId);
    for (const row of targetAgent?.modelHistory ?? []) {
      if (!allowedDates.has(row.date) || row.model !== activeModel.name || !valid(row.totalTokens)) continue;
      const day = chartDaily.get(row.date) ?? { input: 0n, output: 0n, cached: 0n, total: 0n, cost: emptyCost() };
      const inVal = valid(row.inputTokens ?? -1) ? BigInt(row.inputTokens!) : BigInt(row.totalTokens);
      const outVal = valid(row.outputTokens ?? -1) ? BigInt(row.outputTokens!) : 0n;
      const cacheVal = valid(row.cachedTokens ?? -1) ? BigInt(row.cachedTokens!) : 0n;
      day.input += inVal;
      day.output += outVal;
      day.cached += cacheVal;
      day.total += BigInt(row.totalTokens);
      day.cost = addCost(day.cost, estimateModelDayUsd(row));
      chartDaily.set(row.date, day);
    }
  } else {
    for (const [k, v] of daily) chartDaily.set(k, v);
  }

  const chartDates = period === 'all' && chartDaily.size
    ? dates.filter(date => date >= [...chartDaily.keys()].sort()[0])
    : dates;
  const peak = (key: 'input' | 'output') =>
    [...chartDaily.values()].reduce((max, day) => day[key] > max ? day[key] : max, 0n);
  const inputMax = Number(peak('input')), outputMax = Number(peak('output'));
  const bars = chartDates.map(date => {
    const day = chartDaily.get(date);
    const usdLabel = day ? formatEstimatedUsdPlain(day.cost.usd, day.cost.partial) : null;
    const label = day
      ? `${date}: ${number.format(day.input)} ${tr(lang, 'dayInput')}, ${number.format(day.output)} ${tr(lang, 'dayOutput')}${day.cached > 0n ? `, ${number.format(day.cached)} ${tr(lang, 'dayCache')}` : ''}, ${number.format(day.total)} ${tr(lang, 'dayTotal')}${usdLabel ? `, ~${usdLabel}` : ''}`
      : `${date}: ${tr(lang, 'dayNotRecorded')}`;
    return `<li class="${day ? '' : 'unrecorded'}" title="${e(label)}" aria-label="${e(label)}">${day
      ? `<i style="--bar:${Number(day.input) / Math.max(1, inputMax) * 100}%"></i><i style="--bar:${Number(day.output) / Math.max(1, outputMax) * 100}%"></i>` : ''}</li>`;
  }).join('');

  models.sort((a, b) => a.total === b.total ? a.name.localeCompare(b.name) : a.total > b.total ? -1 : 1);
  const metrics = [
    [tr(lang, 'totalTokens'), daily.size ? count(allTotal, unavailableLabel) : count(null, unavailableLabel)],
    [tr(lang, 'inputTokens'), daily.size ? count(allInput, unavailableLabel) : count(null, unavailableLabel)],
    [tr(lang, 'outputTokens'), daily.size ? count(allOutput, unavailableLabel) : count(null, unavailableLabel)],
    [tr(lang, 'cacheTokens'), daily.size ? formatCacheMetric(allCached, allCacheCreation, allCacheRead, lang) : count(null, unavailableLabel)],
    [tr(lang, 'estCostUsd'), daily.size ? formatEstimatedUsd(allCost.usd, allCost.partial, unavailableLabel) : count(null, unavailableLabel)],
    [tr(lang, 'recordedResponses'), countedRows ? count(responses, unavailableLabel) : count(null, unavailableLabel)],
  ];
  const start = chartDates[0];
  const end = dates.at(-1);
  const filterChip = activeModel
    ? `<div class="model-filter-chip"><span>${tr(lang, 'filteringModel', { model: `${e(activeModel.name)} (${e(activeModel.agent)})` })}</span><button type="button" class="clear-filter-btn" data-clear-model="1">${tr(lang, 'clearFilter')}</button></div>`
    : '';
  const daysMeta = chartDaily.size ? tr(lang, 'daysRecorded', { count: chartDaily.size, total: chartDates.length }) : '';
  const usdNote = allCost.partial
    ? `${tr(lang, 'usdEstimateDisclaimer')} ${tr(lang, 'partialEstimateNote')}`
    : tr(lang, 'usdEstimateDisclaimer');

  return `<div class="total-usage">
    <header class="total-heading"><div><h3>${tr(lang, 'agentUsageTitle')}</h3><p class="meta">${period === 'all' ? tr(lang, 'allRetainedMeta') : `${start} → ${end} · UTC`}</p></div>${usagePeriodButtons(period, lang)}</header>
    <dl class="total-metrics">${metrics.map(([label, value]) => `<div><dt>${label}</dt><dd>${value}</dd></div>`).join('')}</dl>
    <p class="meta">${tr(lang, 'totalUsageDisclaimer')}</p>
    <p class="meta">${countedRows < observedRows && countedRows > 0 ? tr(lang, 'partialResponsePrefix') : ''}${tr(lang, 'recordedResponsesNote')} ${usdNote}</p>
    <section class="total-block" aria-labelledby="total-days"><div class="total-heading"><h4 id="total-days">${tr(lang, 'tokensByDay')}</h4>${daysMeta ? `<span class="meta">${daysMeta}</span>` : ''}</div>${filterChip}
      ${chartDaily.size ? `<p class="total-series"><span>${tr(lang, 'inputPeak')} ${short.format(inputMax)}</span><span>${tr(lang, 'outputPeak')} ${short.format(outputMax)}</span></p><ol class="total-bars" aria-label="${tr(lang, 'chartAria')}">${bars}</ol><div class="total-axis"><time>${start}</time><time>${end}</time></div>` : `<p class="meta">${tr(lang, 'noLocalUsage')}</p>`}
    </section>
    <div class="total-breakdowns"><section class="total-block" aria-labelledby="total-models"><div class="total-heading"><h4 id="total-models">${tr(lang, 'modelsTitle')}</h4>${models.length ? `<span class="meta">${tr(lang, 'clickModelHint')}</span>` : ''}</div>${models.length
      ? table('total-models', [tr(lang, 'colModel'), tr(lang, 'colInput'), tr(lang, 'colOutput'), tr(lang, 'colCache'), tr(lang, 'colTotal'), tr(lang, 'colEstUsd')], models.map(row => {
          const pressed = activeModel?.agentId === row.agentId && activeModel?.name === row.name;
          return `<tr><th scope="row"><button type="button" class="model-filter-btn" data-usage-model="${e(row.name)}" data-usage-agent="${e(row.agentId)}" aria-pressed="${pressed}">${e(row.name)}<small>${e(row.agent)}</small></button></th><td>${count(row.input, unavailableLabel)}</td><td>${count(row.output, unavailableLabel)}</td><td>${count(row.cached, unavailableLabel)}</td><td>${count(row.total, unavailableLabel)}</td><td>${formatEstimatedUsd(row.cost.usd, row.cost.partial, unavailableLabel)}</td></tr>`;
        }).join(''))
      : `<p class="meta">${tr(lang, 'noLocalModelUsage')}</p>`}</section>
    <section class="total-block" aria-labelledby="total-agents"><h4 id="total-agents">${tr(lang, 'agentsTitle')}</h4>${agentRows.length
      ? table('total-agents', [tr(lang, 'colAgent'), tr(lang, 'colInput'), tr(lang, 'colOutput'), tr(lang, 'colCache'), tr(lang, 'colTotal'), tr(lang, 'colEstUsd')], agentRows.map(({ agent, input, output, cached, total, cost }) => `<tr><th scope="row"><button type="button" data-section="usage" data-tool="${agent.id}">${e(agent.name)}</button></th><td>${count(input, unavailableLabel)}</td><td>${count(output, unavailableLabel)}</td><td>${count(cached, unavailableLabel)}</td><td>${count(total, unavailableLabel)}</td><td>${formatEstimatedUsd(cost.usd, cost.partial, unavailableLabel)}</td></tr>`).join(''))
      : `<p class="meta">${tr(lang, 'noInstalledTools')}</p>`}</section></div>
  </div>`;
}
