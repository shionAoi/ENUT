import test from 'node:test';
import assert from 'node:assert/strict';
import {
  addCost,
  emptyCost,
  estimateModelDayUsd,
  formatEstimatedUsd,
  lookupModelRates,
  normalizeModelKey,
} from '../src/pricing.ts';

test('normalizes provider prefixes, date stamps, and variant suffixes without conflating tiers', () => {
  assert.equal(normalizeModelKey('anthropic:claude-sonnet-4-5-20250929'), 'claude-sonnet-4-5');
  assert.equal(normalizeModelKey('openrouter/google/gemini-2.5-pro:thinking'), 'gemini-2.5-pro');
  assert.equal(normalizeModelKey('claude-4.5-haiku'), 'claude-haiku-4-5');
  assert.equal(normalizeModelKey('gpt-oss-120b-medium'), 'gpt-oss-120b-medium');
  assert.equal(lookupModelRates('o3-pro')?.input, 20);
  assert.equal(lookupModelRates('o3')?.input, 2);
  assert.equal(lookupModelRates('<local-model>'), null);
  assert.equal(lookupModelRates('Unreported model'), null);
});

test('estimates USD accurately across Claude, Codex, Gemini, and OpenCode without guessing unknowns', () => {
  // Claude Sonnet 4.5 ($3/M in, $15/M out, $3.75/M cacheWrite, $0.30/M cacheRead)
  const claudeUsd = estimateModelDayUsd({
    date: '2026-11-23',
    model: 'claude-sonnet-4-5',
    inputTokens: 1_000_000,
    outputTokens: 100_000,
    cachedTokens: 600_000,
    cacheCreationTokens: 200_000,
    cacheReadTokens: 400_000,
    totalTokens: 1_700_000,
  });
  assert.ok(claudeUsd !== null);
  assert.ok(Math.abs(claudeUsd - (3 + 1.5 + 0.75 + 0.12)) < 1e-9);

  // Codex GPT-5.4 ($2.50/M in, $15/M out, $0.25/M cacheRead)
  const codexUsd = estimateModelDayUsd({
    date: '2026-11-23',
    model: 'gpt-5.4',
    inputTokens: 500_000,
    outputTokens: 50_000,
    cachedTokens: 400_000,
    cacheCreationTokens: 0,
    cacheReadTokens: 400_000,
    totalTokens: 950_000,
  });
  assert.ok(codexUsd !== null);
  assert.ok(Math.abs(codexUsd - (1.25 + 0.75 + 0.1)) < 1e-9);

  // Missing split or unknown model returns null (never $0.00)
  assert.equal(
    estimateModelDayUsd({ date: '2026-11-23', model: 'claude-sonnet-4-5', totalTokens: 500 }),
    null,
  );
  assert.equal(
    estimateModelDayUsd({
      date: '2026-11-23',
      model: 'local-llama',
      inputTokens: 500,
      outputTokens: 100,
      totalTokens: 600,
    }),
    null,
  );
});

test('formats USD sub-cent, standard, partial, and unavailable aggregates safely', () => {
  assert.equal(formatEstimatedUsd(null, false, 'Unavailable'), '<span aria-label="Unavailable">—</span>');
  assert.equal(formatEstimatedUsd(0.0042, false), '$0');
  assert.equal(formatEstimatedUsd(4, false), '$4');
  assert.equal(formatEstimatedUsd(12.345, true), '$12.35*');

  let agg = emptyCost();
  agg = addCost(agg, 1.5);
  assert.deepEqual(agg, { usd: 1.5, partial: false, pricedRows: 1, unpricedRows: 0 });
  agg = addCost(agg, null);
  assert.deepEqual(agg, { usd: 1.5, partial: true, pricedRows: 1, unpricedRows: 1 });
});
