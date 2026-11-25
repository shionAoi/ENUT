import { tr, type Lang } from './i18n.ts';
import type { Agent, BillingReport } from './types.ts';
import { billingTotal, countdown, formatNumber, freshness, money, percent, summarize } from './logic.ts';
import { icon } from './icons.ts';
import { presentAgent, quotaGroups } from './presentation.ts';
import { connectionAction, connectionHint } from './connection-actions.ts';

export type View = 'usage' | 'quotas' | 'models';
const marks: Record<Agent['id'], string> = { codex: 'CX', claude: 'CL', gemini: 'GM', antigravity: 'GM', opencode: 'OC', hermes: 'HE', ollama: 'OL', lmstudio: 'LM' };
export function escapeHtml(value: unknown): string {
  return String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);
}
const e = escapeHtml;
function metric(label: string, value: string, note: string, extra = ''): string {
  return `<article class="panel metric"><p class="metric-label">${label}</p><div class="metric-line"><strong>${value}</strong>${extra}</div><p class="meta">${note}</p></article>`;
}
export function renderSummary(agents: Agent[], reports: BillingReport[] = []): string {
  agents=agents.map(presentAgent);
  const sum = summarize(agents);
  const spend = reports.length ? billingTotal(reports) : sum.spend;
  const partial = agents.filter(a => a.installed && a.connected && a.partialUsage).map(a => a.partialUsage!.inputTokens).filter(n => Number.isSafeInteger(n) && n >= 0);
  const inputOnly = sum.tokens === null && partial.length > 0;
  return metric(inputOnly ? 'Observed input tokens' : 'Observed tokens', formatNumber(inputOnly ? partial.reduce((a,b)=>a+b,0) : sum.tokens), inputOnly ? 'Recorded input/cache only · output and total unavailable' : sum.tokenSources ? `This computer · ${sum.tokenSources} reporting source${sum.tokenSources === 1 ? '' : 's'} · recorded source periods` : 'This computer · no observed usage yet')
    + metric('Reported API spend', money(spend), spend === null ? 'Connect organization reporting separately' : 'Organization reports · USD · month-to-date · reporting delays apply')
    + metric('Next quota reset', countdown(sum.reset), sum.reset ? `${e(new Date(sum.reset).toLocaleString())} · provider reported` : 'No supported reset time reported');
}
export function renderBilling(agents: Agent[], reports: BillingReport[], desktop: boolean, lang: Lang = 'en'): string {
  if (!desktop) return '';
  const providers = agents.filter(a => a.installed && (a.id === 'codex' || a.id === 'claude'));
  if (!providers.length) return '';
  return `<section class="panel billing-panel" aria-labelledby="billing-title"><h2 id="billing-title">${tr(lang, 'apiReportingTitle')}</h2><p class="meta">${tr(lang, 'apiReportingDesc')}</p>${providers.map(agent => {
    const provider = agent.id === 'codex' ? 'openai' : 'anthropic';
    const name = provider === 'openai' ? 'OpenAI' : 'Anthropic';
    const report = reports.find(r => r.provider === provider);
    return `<div class="billing-row"><div><strong>${name}</strong><p class="meta">${report ? `${money(report.spendUsd, lang)} · ${e(freshness(report.asOf, Date.now(), lang))}` : tr(lang, 'notConnected')}</p>${report ? `<p class="meta">${e(report.message)}</p>` : ''}</div><button type="button" data-billing="${provider}" data-billing-action="${report ? 'disconnect' : 'connect'}">${report ? tr(lang, 'disconnect') : tr(lang, 'connectProviderReporting', { provider: name })}</button></div>`;
  }).join('')}</section>`;
}
export function quotas(agent: Agent, now = Date.now(), lang: Lang = 'en'): string {
  agent=presentAgent(agent);
  if (agent.kind === 'local' && !agent.quotas.length) return `<p class="unavailable">${tr(lang, 'noSubscriptionQuota')}</p><p class="meta">${lang === 'es' ? 'El inventario de entorno local no es una cuota de cuenta.' : 'Local runtime inventory is not an account allowance.'}</p>`;
  if (!agent.quotas.length) return `<p class="unavailable">${tr(lang, 'allowanceUnavailable')}</p><p class="meta">${tr(lang, 'allowanceUnsupported')}</p>`;
  return agent.quotas.map(q => {
    const remaining = percent(q.remainingPercent);
    return `<div class="quota"><div class="data-line"><span>${e(q.label)}</span><strong>${remaining === null ? tr(lang, 'unavailable') : `${formatNumber(remaining)}${tr(lang, 'remainingSuffix')}`}</strong></div>
      ${remaining === null ? '' : `<progress value="${remaining}" max="100" aria-label="${e(q.label)} ${e(tr(lang, 'remainingAllowanceAria'))}" aria-valuetext="${formatNumber(remaining)}${tr(lang, 'remainingSuffix')}">${formatNumber(remaining)}%</progress>`}
      <p class="meta">${tr(lang, 'resetPrefix')} ${e(countdown(q.resetsAt,now,lang))}${q.resetsAt && Number.isFinite(Date.parse(q.resetsAt)) ? ` · <time datetime="${e(q.resetsAt)}">${e(new Date(q.resetsAt).toISOString())}</time>` : ''}</p><p class="meta">${tr(lang, 'sourcePrefix')} ${e(q.source)}</p></div>`;
  }).join('');
}
export function usage(agent: Agent): string {
  const u = agent.usage;
  if (!u && agent.partialUsage) {
    const partial=agent.partialUsage;
    return `<dl class="usage-grid"><div><dt>Recorded input</dt><dd>${formatNumber(partial.inputTokens)}</dd></div><div><dt>Cached input</dt><dd>${formatNumber(partial.cachedTokens)}</dd></div><div><dt>Output</dt><dd>Unavailable</dd></div><div><dt>Total tokens</dt><dd>Unavailable</dd></div></dl><p class="meta">Cache reads are included in input, not added twice.</p><p class="meta">${e(partial.note)}</p><p class="meta">${e(partial.source)} · ${freshness(partial.asOf)}</p><p class="meta">Recorded period: ${e(partial.periodStart.slice(0,10))} → ${e(partial.periodEnd.slice(0,10))} UTC. Not a subscription balance.</p>`;
  }
  if (!u) return '<p class="unavailable">Usage unavailable</p><p class="meta">No supported usage metadata reported. Missing values are not zero.</p>';
  return `<dl class="usage-grid"><div><dt>Total tokens</dt><dd>${formatNumber(u.totalTokens)}</dd></div><div><dt>Input</dt><dd>${formatNumber(u.inputTokens)}</dd></div><div><dt>Output</dt><dd>${formatNumber(u.outputTokens)}</dd></div><div><dt>Cached</dt><dd>${formatNumber(u.cachedTokens)}</dd></div></dl>
    <p class="meta">${u.scope === 'account' ? 'Account-wide' : 'This computer'} · Source: ${e(u.source)}</p>
    <p class="meta">${freshness(u.asOf)} · ${u.periodStart && u.periodEnd ? `${e(u.periodStart.slice(0,10))} → ${e(u.periodEnd.slice(0,10))} UTC · recorded subset` : 'Source period, not a daily total'}</p>
    <div class="data-line small-line"><span>Reported spend (USD)</span><strong>${money(u.spendUsd)}</strong></div>${agent.partialUsage ? `<div class="connector-note"><strong>Separate ordinary transcript counters</strong><p>Recorded input: ${formatNumber(agent.partialUsage.inputTokens)} · cached input: ${formatNumber(agent.partialUsage.cachedTokens)}</p><p class="meta">Not added to the complete totals above. Ordinary output/total remains unknown; counters may overlap supported records.</p></div>` : ''}`;
}
function models(agent: Agent): string {
  if (!agent.models.length) return `<p class="unavailable">No models reported</p><p class="meta">${agent.kind === 'local' ? 'A stopped runtime may not expose its model inventory.' : 'Cloud models are not downloaded local models.'}</p>`;
  return `<ul class="model-list">${agent.models.map(m => {const count=agent.modelUsage?.find(item=>item.model===m.id)?.totalTokens; return `<li><div><strong>${e(m.name)}</strong><span class="meta">${agent.kind === 'local' ? 'Local model' : 'Observed cloud model'} · ${m.loaded === true ? 'Loaded' : m.loaded === false ? 'Not loaded' : 'Load state unknown'}</span></div><span class="model-size">${count !== undefined ? `${formatNumber(count)} recorded tokens` : m.sizeBytes !== null && Number.isFinite(m.sizeBytes) && m.sizeBytes >= 0 ? `${formatNumber(m.sizeBytes / 1024 ** 3)} GB` : 'Size unknown'}</span></li>`}).join('')}</ul>`;
}

export function renderHistory(agents: Agent[]): string {
  const days=new Map<string,number>();
  for (const agent of agents.filter(a=>a.installed&&a.connected)) for (const day of agent.usageHistory??[]) {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(day.date)||!Number.isSafeInteger(day.totalTokens)||day.totalTokens<0) continue;
    const sum=(days.get(day.date)??0)+day.totalTokens;
    if (Number.isSafeInteger(sum)) days.set(day.date,sum);
  }
  const rows=[...days].sort(([a],[b])=>a.localeCompare(b)).slice(-30);
  const max=Math.max(1,...rows.map(([,n])=>n));
  return `<section class="panel history-panel" aria-labelledby="history-title"><div class="section-heading"><div><h2 id="history-title">Recorded token history</h2><p class="meta">Last 30 UTC days · supported saved records only</p></div></div>${rows.length ? `<div class="history-scroll"><ol class="history-bars">${rows.map(([date,n])=>`<li><span class="history-count">${formatNumber(n)} tokens</span><span class="history-track"><span style="height:${n/max*100}%"></span></span><time datetime="${date}">${date.slice(5)}</time></li>`).join('')}</ol></div>` : '<p class="unavailable">No recorded token history yet</p>'}<p class="meta">Missing dates and unsupported sources are unknown, not zero. Saved transcripts can omit deleted or rewound turns; this is not a billed or lifetime ledger.</p></section>`;
}
export function renderAgents(agents: Agent[], view: View, desktop: boolean, approvedIds?: Set<string>, lang: Lang = 'en'): string {
  agents=agents.map(presentAgent);
  if (!agents.length) return `<div class="panel empty"><div class="empty-symbol">${icon('desktop')}</div><h3>${desktop ? tr(lang, 'noMatchingTitle') : tr(lang, 'desktopRequiredTitle')}</h3><p>${desktop ? tr(lang, 'noMatchingBody') : tr(lang, 'desktopRequiredBody')}</p><div class="empty-foot">${icon('shield')}<span>${tr(lang, 'noAccessFoot')}</span></div></div>`;
  return agents.map(a => {
    const approved = approvedIds ? approvedIds.has(a.id) : a.connected;
    return `<article class="panel agent-card" aria-labelledby="agent-${a.id}">
    <header class="agent-header"><div class="agent-identity">${icon(a.kind)}<div><h3 id="agent-${a.id}">${e(a.name)}</h3><span class="meta">${a.kind === 'local' ? (lang === 'es' ? 'Entorno local' : 'Local runtime') : (lang === 'es' ? 'App en la nube' : 'Cloud app')} · ${lang === 'es' ? 'Instalado' : 'Installed'}</span></div></div>
    <span class="status ${approved && a.status === 'ready' ? 'healthy' : ''}">${!approved ? tr(lang, 'notConnected') : a.status === 'ready' ? tr(lang, 'connected') : a.status === 'error' ? 'Error' : tr(lang, 'unavailable')}</span></header>
    <div class="runtime-line"><span>${a.running === true ? tr(lang, 'running') : a.running === false ? (lang === 'es' ? 'Detenido' : 'Stopped') : (lang === 'es' ? 'Actividad desconocida' : 'Activity unknown')}</span><span>${approved ? freshness(a.updatedAt, Date.now(), lang) : tr(lang, 'permissionRequired')}</span></div>
    <div class="agent-content">${!approved ? `<p class="unavailable">${lang === 'es' ? 'Tu permiso es primero' : 'Your permission comes first'}</p><p class="meta">${lang === 'es' ? 'Conecta esta integración para leer uso compatible, cuotas o inventario de modelos.' : 'Connect this integration to read supported usage, quotas or model inventory.'}</p>` : view === 'quotas' ? quotas(a, Date.now(), lang) : view === 'models' ? models(a) : usage(a)}</div>
    ${approved && a.message ? `<p class="connector-note">${e(a.message)}</p>` : ''}
    <footer class="agent-footer"><span class="meta">${approved ? (lang === 'es' ? 'Acceso de monitoreo recordado' : 'Remembered monitoring access') : (lang === 'es' ? 'Sin acceso a datos de cuenta' : 'No account data accessed')}</span><button type="button" class="text-button" data-action="${approved ? 'revoke' : 'connect'}" data-agent="${a.id}">${approved ? tr(lang, 'revokeAccess') : `${tr(lang, 'connectLabel')} ${icon('arrow')}`}</button></footer>
  </article>`;
  }).join('');
}

function compactQuotas(agent: Agent, lang: Lang = 'en'): string {
  if(!agent.quotas.length)return `<strong class="tool-value">${agent.kind==='local'?tr(lang,'noSubscriptionQuota'):tr(lang,'quotaUnavailable')}</strong>`;
  return quotaGroups(agent).map(({group,rows})=>`<div class="quota-group">${group?`<h4>${e(group)}</h4>`:''}<dl class="quota-windows">${rows.map(({quota,label})=>{
    const remaining=percent(quota.remainingPercent);
    return `<div><dt>${e(label)}</dt><dd>${remaining===null?tr(lang,'unavailable'):`${formatNumber(remaining)}${tr(lang,'leftSuffix')}`}</dd></div>`;
  }).join('')}</dl></div>`).join('');
}

function toolQuotaDetails(agent: Agent, now: number, lang: Lang = 'en'): string {
  if (!agent.quotas.length) return quotas(agent, now, lang);
  return agent.quotas.map(q => {
    const remaining = percent(q.remainingPercent);
    const reset = q.resetsAt ? Date.parse(q.resetsAt) : NaN;
    const label = `${formatNumber(remaining)}${tr(lang,'remainingSuffix')}`;
    return `<div class="quota tool-quota">
      ${remaining === null ? '' : `<div class="quota-ring" style="--remaining:${remaining}%">
        <meter class="sr-only" min="0" max="100" value="${remaining}" aria-label="${e(q.label)} ${e(tr(lang, 'remainingAllowanceAria'))}" aria-valuetext="${label}"></meter>
        <strong aria-hidden="true">${label}</strong>
      </div>`}
      <div class="tool-quota-copy"><strong>${e(q.label)}</strong>
        ${remaining === null ? `<p class="meta">${tr(lang, 'allowanceUnavailable')}</p>` : ''}
        <p class="meta">${tr(lang, 'resetPrefix')} ${e(countdown(q.resetsAt, now, lang))}${Number.isFinite(reset) ? ` · <time datetime="${e(q.resetsAt)}">${e(new Date(reset).toISOString())}</time>` : ''}</p>
        <p class="meta">${tr(lang, 'sourcePrefix')} ${e(q.source)}</p>
      </div>
    </div>`;
  }).join('');
}

export function renderTools(agents: Agent[], view: View, desktop: boolean, approved: Set<string>, expanded: Set<string>, now = Date.now(), lang: Lang = 'en'): string {
  const installed=agents.filter(agent=>agent.installed).map(presentAgent);
  if (!installed.length) return renderAgents([],view,desktop,approved,lang);
  return `<div class="tool-list">${installed.map(agent=>{
    const allowed=approved.has(agent.id);
    const open=expanded.has(agent.id);
    const action=connectionAction(agent,approved,lang);
    const help=action.action==='revoke'?tr(lang,'revokeHelp'):connectionHint(agent,lang);
    const live=allowed&&agent.connected;
    const summary=live?compactQuotas(agent,lang):`<strong class="tool-value">${allowed?tr(lang,'unavailable'):tr(lang,'permissionRequired')}</strong>`;
    const state=agent.running===true?tr(lang,'running'):live?tr(lang,'connected'):allowed?tr(lang,'unavailable'):tr(lang,'detected');
    const content=!allowed?`<p class="unavailable">${tr(lang,'permissionRequired')}</p><p class="meta">${tr(lang,'connectBeforeReading')}</p>`:!agent.connected?`<p class="unavailable">${tr(lang,'quotaUnavailable')}</p><p class="meta">${tr(lang,'quotaUnavailableBody')}</p>`:toolQuotaDetails(agent,now,lang);
    return `<article class="tool-row" aria-labelledby="tool-${agent.id}"><div class="tool-row-top">
      <span class="tool-mark mark-${agent.id}" aria-hidden="true">${marks[agent.id]}</span>
      <div class="tool-info"><div class="tool-name-line"><h3 id="tool-${agent.id}">${e(agent.name)}</h3><span class="tool-state ${agent.running===true?'running':''}">${state}</span>
        <span class="quota-help-wrap"><button type="button" class="quota-help" id="tool-help-${agent.id}" aria-label="${e(tr(lang, 'aboutActionForTool', { action: action.label, tool: agent.name }))}" aria-describedby="help-${agent.id}">?</button>
        <span class="quota-tooltip" id="help-${agent.id}" role="tooltip">${e(help)}</span></span>
      </div></div>
      <button class="tool-connect ${action.action==='revoke'?'':'primary'}" type="button" data-action="${action.action}" data-agent="${agent.id}" ${desktop?'':'disabled'}>${action.label}</button></div>
      <div class="quota-summary">${summary}<button type="button" class="quota-details-toggle" data-action="details" data-agent="${agent.id}" aria-label="${e(tr(lang, 'quotaDetailsForTool', { tool: agent.name }))}" aria-expanded="${open}" aria-controls="detail-${agent.id}">${tr(lang,'quotaDetails')}</button></div>
      <div id="detail-${agent.id}" class="tool-details" ${open?'':'hidden'}>${content}
        ${allowed&&action.action!=='revoke'?`<button type="button" data-action="revoke" data-agent="${agent.id}" ${desktop?'':'disabled'}>${tr(lang,'revokeAccess')}</button>`:''}
      </div></article>`;
  }).join('')}</div>`;
}
