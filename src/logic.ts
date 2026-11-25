import type { Lang } from './i18n.ts';
import type { Agent, BillingReport, KindFilter, Theme, Usage } from './types.ts';

export function resolveTheme(saved: string | null, darkOS: boolean): Theme {
  return saved === 'light' || saved === 'dark' ? saved : darkOS ? 'dark' : 'light';
}
export function saveTheme(storage: Pick<Storage, 'setItem'>, theme: Theme): void {
  try { storage.setItem('ai-monitor-theme', theme); } catch { /* Storage may be unavailable in a webview. */ }
}
export function selectAgents(agents: Agent[], filter: KindFilter): Agent[] {
  return agents.filter(a => a.installed && (filter === 'all' || a.kind === filter));
}
const valid = (n: number | null): n is number => n !== null && Number.isFinite(n) && n >= 0;
export function percent(n: number | null): number | null { return valid(n) && n <= 100 ? n : null; }
const numberFormatter = new Intl.NumberFormat('en-US', { maximumFractionDigits: 2 });
const usdFormatter = new Intl.NumberFormat('en-US', {
  style: 'currency', currency: 'USD', minimumFractionDigits: 0, maximumFractionDigits: 2,
});
export function formatUsd(n: number): string { return usdFormatter.format(n); }
export function formatNumber(n: number | null): string {
  return valid(n) ? numberFormatter.format(n) : 'Unknown';
}
export function money(n: number | null, lang: Lang = 'en'): string {
  return n !== null && Number.isFinite(n)
    ? formatUsd(n)
    : lang === 'es' ? 'No disponible' : 'Unavailable';
}
export function billingTotal(reports: BillingReport[]): number | null {
  const unique = new Map(reports.map(report => [report.provider, report]));
  const known = [...unique.values()].map(r => r.spendUsd).filter((n): n is number => n !== null && Number.isFinite(n));
  return known.length ? known.reduce((a,b) => a + b, 0) : null;
}
export function countdown(timestamp: string | null, now = Date.now(), lang: Lang = 'en'): string {
  const end = timestamp === null ? NaN : Date.parse(timestamp);
  if (!Number.isFinite(end)) return lang === 'es' ? 'No disponible' : 'Unavailable';
  const minutes = Math.floor((end - now) / 60000);
  if (end <= now) return lang === 'es' ? 'Reinicio pendiente' : 'Reset pending';
  if (minutes < 1) return '<1m';
  if (minutes < 60) return `${minutes}m`;
  if (minutes < 1440) return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
  return `${Math.floor(minutes / 1440)}d ${Math.floor(minutes % 1440 / 60)}h`;
}
export function freshness(timestamp: string | null, now = Date.now(), lang: Lang = 'en'): string {
  if (!timestamp || !Number.isFinite(Date.parse(timestamp))) return lang === 'es' ? 'Sin actualizar' : 'Not updated';
  const age = Math.max(0, Math.floor((now - Date.parse(timestamp)) / 1000));
  if (age < 60) return lang === 'es' ? 'Actualizado ahora mismo' : 'Updated just now';
  if (age < 3600) {
    const mins = Math.floor(age / 60);
    return lang === 'es'
      ? `Actualizado hace ${mins}m${age >= 180 ? ' · desactualizado' : ''}`
      : `Updated ${mins}m ago${age >= 180 ? ' · stale' : ''}`;
  }
  const stale = lang === 'es' ? 'desactualizado' : 'stale';
  const prefix = lang === 'es' ? 'Actualizado' : 'Updated';
  return `${prefix} ${new Date(timestamp).toLocaleString()} · ${stale}`;
}
export function pollingDelay(failures: number): number { return Math.min(45000 * 2 ** failures, 360000); }

// The seam has no day/window field: these are source-period observations, never "today".
// Deduplicate identical account reports by source; device totals must never enter account spend.
export function summarize(agents: Agent[], now = Date.now()) {
  const approved = agents.filter(a => a.installed && a.connected);
  const device: Usage[] = [];
  const accounts = new Map<string, Usage>();
  for (const a of approved) {
    if (!a.usage) continue;
    if (a.usage.scope === 'device') device.push(a.usage);
    else {
      const old = accounts.get(a.usage.source);
      if (!old || Date.parse(a.usage.asOf) > Date.parse(old.asOf)) accounts.set(a.usage.source, a.usage);
    }
  }
  const sum = (values: (number | null)[]) => {
    const known = values.filter(valid);
    return known.length ? known.reduce((a, b) => a + b, 0) : null;
  };
  const resets = approved.flatMap(a => a.quotas.map(q => q.resetsAt))
    .filter((s): s is string => s !== null && Number.isFinite(Date.parse(s)) && Date.parse(s) > now)
    .sort((a, b) => Date.parse(a) - Date.parse(b));
  return {
    tokens: sum(device.map(u => u.totalTokens)),
    accountTokens: sum([...accounts.values()].map(u => u.totalTokens)),
    spend: sum([...accounts.values()].map(u => u.spendUsd)),
    reset: resets[0] ?? null,
    tokenSources: device.filter(u => valid(u.totalTokens)).length,
  };
}
