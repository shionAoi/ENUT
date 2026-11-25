import type { ModelDayUsage } from './types.ts';
import { formatUsd } from './logic.ts';

/** Standard API list rates in USD per 1,000,000 tokens. */
export interface ModelRates {
  input: number;
  output: number;
  cacheWrite: number;
  cacheRead: number;
}

const MODEL_RATES: Record<string, ModelRates> = {
  // Anthropic Claude
  'claude-opus-4-6': { input: 5, output: 25, cacheWrite: 6.25, cacheRead: 0.5 },
  'claude-opus-4-5': { input: 5, output: 25, cacheWrite: 6.25, cacheRead: 0.5 },
  'claude-opus-5-5': { input: 5, output: 25, cacheWrite: 6.25, cacheRead: 0.5 },
  'claude-opus-4-1': { input: 15, output: 75, cacheWrite: 18.75, cacheRead: 1.5 },
  'claude-opus-4': { input: 15, output: 75, cacheWrite: 18.75, cacheRead: 1.5 },
  'claude-3-opus': { input: 15, output: 75, cacheWrite: 18.75, cacheRead: 1.5 },
  'claude-sonnet-4-6': { input: 3, output: 15, cacheWrite: 3.75, cacheRead: 0.3 },
  'claude-sonnet-4-5': { input: 3, output: 15, cacheWrite: 3.75, cacheRead: 0.3 },
  'claude-sonnet-4': { input: 3, output: 15, cacheWrite: 3.75, cacheRead: 0.3 },
  'claude-3-7-sonnet': { input: 3, output: 15, cacheWrite: 3.75, cacheRead: 0.3 },
  'claude-3-5-sonnet': { input: 3, output: 15, cacheWrite: 3.75, cacheRead: 0.3 },
  'claude-haiku-4-5': { input: 1, output: 5, cacheWrite: 1.25, cacheRead: 0.1 },
  'claude-3-5-haiku': { input: 0.8, output: 4, cacheWrite: 1, cacheRead: 0.08 },
  'claude-3-haiku': { input: 0.25, output: 1.25, cacheWrite: 0.3, cacheRead: 0.03 },

  // OpenAI GPT / Codex / o-series
  'gpt-5.4': { input: 2.5, output: 15, cacheWrite: 0, cacheRead: 0.25 },
  'gpt-5.6-luna': { input: 2.5, output: 15, cacheWrite: 0, cacheRead: 0.25 },
  'gpt-5.3-codex': { input: 1.75, output: 14, cacheWrite: 0, cacheRead: 0.175 },
  'gpt-5.2-codex': { input: 1.75, output: 14, cacheWrite: 0, cacheRead: 0.175 },
  'gpt-5.2': { input: 1.75, output: 14, cacheWrite: 0, cacheRead: 0.175 },
  'gpt-5.1-codex-mini': { input: 0.25, output: 2, cacheWrite: 0, cacheRead: 0.025 },
  'gpt-5.1-codex': { input: 1.25, output: 10, cacheWrite: 0, cacheRead: 0.125 },
  'gpt-5.1': { input: 1.25, output: 10, cacheWrite: 0, cacheRead: 0.125 },
  'gpt-5-codex': { input: 1.25, output: 10, cacheWrite: 0, cacheRead: 0.125 },
  'gpt-5-mini': { input: 0.25, output: 2, cacheWrite: 0, cacheRead: 0.025 },
  'gpt-5-nano': { input: 0.05, output: 0.4, cacheWrite: 0, cacheRead: 0.005 },
  'gpt-5': { input: 1.25, output: 10, cacheWrite: 0, cacheRead: 0.125 },
  'gpt-4.1': { input: 2, output: 8, cacheWrite: 0, cacheRead: 0.5 },
  'gpt-4.1-mini': { input: 0.4, output: 1.6, cacheWrite: 0, cacheRead: 0.1 },
  'gpt-4.1-nano': { input: 0.1, output: 0.4, cacheWrite: 0, cacheRead: 0.025 },
  'gpt-4o': { input: 2.5, output: 10, cacheWrite: 0, cacheRead: 1.25 },
  'gpt-4o-mini': { input: 0.15, output: 0.6, cacheWrite: 0, cacheRead: 0.075 },
  'o4-mini': { input: 1.1, output: 4.4, cacheWrite: 0, cacheRead: 0.275 },
  'o3-mini': { input: 1.1, output: 4.4, cacheWrite: 0, cacheRead: 0.55 },
  'o3-pro': { input: 20, output: 80, cacheWrite: 0, cacheRead: 20 },
  o3: { input: 2, output: 8, cacheWrite: 0, cacheRead: 0.5 },
  o1: { input: 15, output: 60, cacheWrite: 0, cacheRead: 7.5 },
  'codex-mini-latest': { input: 1.5, output: 6, cacheWrite: 0, cacheRead: 0.375 },
  'gpt-oss-120b-medium': { input: 0.15, output: 0.6, cacheWrite: 0, cacheRead: 0.0375 },

  // Google Gemini
  'gemini-2.5-pro': { input: 1.25, output: 10, cacheWrite: 0, cacheRead: 0.31 },
  'gemini-3-pro': { input: 2, output: 12, cacheWrite: 0, cacheRead: 0.2 },
  'gemini-3.1-pro': { input: 2, output: 12, cacheWrite: 0, cacheRead: 0.2 },
  'gemini-3.6-pro': { input: 2, output: 12, cacheWrite: 0, cacheRead: 0.2 },
  'gemini-3.7-pro': { input: 2, output: 12, cacheWrite: 0, cacheRead: 0.2 },
  'gemini-4-argon': { input: 2, output: 12, cacheWrite: 0, cacheRead: 0.2 },
  'gemini-4-neon': { input: 2, output: 12, cacheWrite: 0, cacheRead: 0.2 },
  'gemini-2.5-flash': { input: 0.3, output: 2.5, cacheWrite: 0, cacheRead: 0.075 },
  'gemini-3-flash': { input: 0.5, output: 3, cacheWrite: 0, cacheRead: 0.05 },
  'gemini-3.5-flash': { input: 0.5, output: 3, cacheWrite: 0, cacheRead: 0.05 },
  'gemini-3.6-flash': { input: 0.5, output: 3, cacheWrite: 0, cacheRead: 0.05 },
  'gemini-3.7-flash': { input: 0.5, output: 3, cacheWrite: 0, cacheRead: 0.05 },
  'gemini-3.8-flash': { input: 0.5, output: 3, cacheWrite: 0, cacheRead: 0.05 },
  'gemini-2.5-flash-lite': { input: 0.1, output: 0.4, cacheWrite: 0, cacheRead: 0.025 },
  'gemini-3.1-flash-lite': { input: 0.25, output: 1.5, cacheWrite: 0, cacheRead: 0.025 },
  'gemini-2.0-flash': { input: 0.1, output: 0.4, cacheWrite: 0, cacheRead: 0.025 },
  'gemini-2.0-flash-lite': { input: 0.075, output: 0.3, cacheWrite: 0, cacheRead: 0.01875 },
  'gemini-1.5-pro': { input: 1.25, output: 5, cacheWrite: 0, cacheRead: 0.3125 },
  'gemini-1.5-flash': { input: 0.075, output: 0.3, cacheWrite: 0, cacheRead: 0.01875 },
};

const safeNonNeg = (n: number | undefined): n is number =>
  n !== undefined && Number.isSafeInteger(n) && n >= 0;

export function normalizeModelKey(raw: string): string {
  let key = raw.trim().toLowerCase().replace(/\s+/g, '-');
  const slash = key.lastIndexOf('/');
  if (slash !== -1) key = key.slice(slash + 1);
  const colon = key.indexOf(':');
  if (colon !== -1 && key.slice(colon + 1) !== 'thinking') {
    key = key.slice(colon + 1);
  }
  key = key.replace(/:thinking$/, '');
  if (MODEL_RATES[key]) return key;

  // Strip trailing date stamps (-YYYYMMDD or -YYYY-MM-DD)
  key = key.replace(/-\d{4}-\d{2}-\d{2}$/, '').replace(/-\d{8}$/, '');

  // Convert Anthropic "claude-4.5-sonnet" / "claude-sonnet-4.6" to "claude-sonnet-4-5"
  const claudeReversed = key.match(/^claude-(\d+(?:[.-]\d+)?)-(opus|sonnet|haiku)(.*)$/);
  if (claudeReversed) {
    key = `claude-${claudeReversed[2]}-${claudeReversed[1].replace(/\./g, '-')}${claudeReversed[3]}`;
  } else if (key.startsWith('claude-')) {
    key = key.replace(/\./g, '-');
  }
  if (MODEL_RATES[key]) return key;

  for (const suffix of ['-preview', '-latest', '-thinking', '-extra-low', '-high', '-medium', '-low']) {
    if (key.endsWith(suffix)) {
      const stripped = key.slice(0, -suffix.length);
      if (MODEL_RATES[stripped]) return stripped;
      key = stripped;
    }
  }
  return key.replace(/-\d{4}-\d{2}-\d{2}$/, '').replace(/-\d{8}$/, '');
}

export function lookupModelRates(model: string): ModelRates | null {
  const key = normalizeModelKey(model);
  return MODEL_RATES[key] ?? null;
}

export function estimateModelDayUsd(row: ModelDayUsage): number | null {
  if (!safeNonNeg(row.inputTokens) || !safeNonNeg(row.outputTokens)) return null;
  const rates = lookupModelRates(row.model);
  if (!rates) return null;
  const cacheWrite = safeNonNeg(row.cacheCreationTokens) ? row.cacheCreationTokens : 0;
  const cacheRead = safeNonNeg(row.cacheReadTokens)
    ? row.cacheReadTokens
    : safeNonNeg(row.cachedTokens)
      ? Math.max(0, row.cachedTokens - cacheWrite)
      : 0;
  return (
    row.inputTokens * rates.input +
    row.outputTokens * rates.output +
    cacheWrite * rates.cacheWrite +
    cacheRead * rates.cacheRead
  ) / 1_000_000;
}

export interface CostAggregate {
  usd: number | null;
  partial: boolean;
  pricedRows: number;
  unpricedRows: number;
}

export function emptyCost(): CostAggregate {
  return { usd: null, partial: false, pricedRows: 0, unpricedRows: 0 };
}

export function addCost(agg: CostAggregate, value: number | null): CostAggregate {
  if (value === null || !Number.isFinite(value) || value < 0) {
    const unpricedRows = agg.unpricedRows + 1;
    return { ...agg, unpricedRows, partial: agg.pricedRows > 0 && unpricedRows > 0 };
  }
  const pricedRows = agg.pricedRows + 1;
  return {
    usd: (agg.usd ?? 0) + value,
    pricedRows,
    unpricedRows: agg.unpricedRows,
    partial: pricedRows > 0 && agg.unpricedRows > 0,
  };
}

export function mergeCost(a: CostAggregate, b: CostAggregate): CostAggregate {
  const pricedRows = a.pricedRows + b.pricedRows;
  const unpricedRows = a.unpricedRows + b.unpricedRows;
  return {
    usd: pricedRows > 0 ? (a.usd ?? 0) + (b.usd ?? 0) : null,
    pricedRows,
    unpricedRows,
    partial: pricedRows > 0 && unpricedRows > 0,
  };
}

export function formatEstimatedUsdPlain(usd: number | null, partial = false): string | null {
  if (usd === null || !Number.isFinite(usd) || usd < 0) return null;
  const base = formatUsd(usd);
  return partial ? `${base}*` : base;
}

export function formatEstimatedUsd(
  usd: number | null,
  partial = false,
  unavailableLabel = 'Unavailable',
): string {
  const plain = formatEstimatedUsdPlain(usd, partial);
  return plain === null ? `<span aria-label="${unavailableLabel}">—</span>` : plain;
}
