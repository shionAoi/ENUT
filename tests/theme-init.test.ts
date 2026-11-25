import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';

const script = readFileSync(new URL('../public/theme-init.js', import.meta.url), 'utf8');
test('pre-paint script applies saved choice, OS fallback, and unavailable-storage fallback', () => {
  for (const [saved, darkOS, expected] of [
    ['light', true, 'light'], ['dark', false, 'dark'], [null, true, 'dark'],
    [null, false, 'light'], ['invalid', true, 'dark'], ['throws', false, 'light'],
  ]) {
    const document = { documentElement: { dataset: {} as Record<string, string> } };
    runInNewContext(script, {
      document,
      window: { matchMedia: () => ({ matches: darkOS }) },
      get localStorage() {
        if (saved === 'throws') throw Error('Storage unavailable');
        return { getItem: () => saved };
      },
    });
    assert.equal(document.documentElement.dataset.theme, expected);
  }
});
