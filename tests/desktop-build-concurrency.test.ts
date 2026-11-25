import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, delimiter, dirname, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const wrapper = fileURLToPath(new URL('../scripts/desktop-build.mjs', import.meta.url));
const outputs = (stdout: string) => [...stdout.matchAll(/^Installer: (.+)$/gm)]
  .map(match => basename(match[1])).sort();

for (const first of ['deb', 'no-bundle']) {
  test(`overlapping ${first} build never exports the other RPM`, {
    skip: process.platform === 'win32',
  }, async t => {
    const cwd = mkdtempSync(join(tmpdir(), 'desktop-overlap-'));
    t.after(() => rmSync(cwd, { recursive: true, force: true }));
    mkdirSync(join(cwd, 'bin'));
    writeFileSync(join(cwd, 'bin', 'tauri'), '#!/bin/sh\nexec node "$0.cjs" "$@"\n', { mode: 0o755 });
    writeFileSync(join(cwd, 'bin', 'tauri.cjs'), `
      const fs = require('node:fs'), path = require('node:path');
      (async () => {
        const kind = process.argv.at(-1);
        if (kind !== 'rpm') {
          fs.writeFileSync('started', '');
          while (!fs.existsSync('resume')) await new Promise(r => setTimeout(r, 10));
        }
        if (kind === '--no-bundle') return;
        const dir = path.resolve('src-tauri', process.env.CARGO_TARGET_DIR || 'target',
          'release', 'bundle', kind);
        fs.mkdirSync(dir, { recursive: true });
        fs.writeFileSync(path.join(dir, 'monitor.' + kind), kind);
      })();
    `);
    const run = (...args: string[]) => promisify(execFile)(process.execPath, [wrapper, ...args], {
      cwd, timeout: 10_000,
      env: { ...process.env, CARGO_TARGET_DIR: 'target', CARGO_BUILD_TARGET: '',
        PATH: [join(cwd, 'bin'), dirname(process.execPath), process.env.PATH].join(delimiter) },
    });
    const a = run(...(first === 'deb' ? ['--bundles', 'deb'] : ['--no-bundle']));
    for (let i = 0; i < 200 && !existsSync(join(cwd, 'started')); i++) await delay(10);
    const started = existsSync(join(cwd, 'started'));
    const b = run('--bundles', 'rpm');
    try {
      await Promise.race([b, delay(1500, undefined, { ref: false })]);
    } finally {
      writeFileSync(join(cwd, 'resume'), '');
    }
    const [left, right] = await Promise.all([a, b]);
    assert.ok(started);
    assert.deepEqual(outputs(left.stdout), first === 'deb' ? ['monitor.deb'] : []);
    assert.deepEqual(outputs(right.stdout), ['monitor.rpm']);
  });
}
