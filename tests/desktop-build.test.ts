import test from 'node:test';
import assert from 'node:assert/strict';
import { copyFileSync, existsSync, mkdtempSync, mkdirSync, writeFileSync, readdirSync, readFileSync, rmSync, symlinkSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { bundleDirForArgs, exportBundles, resolveTauriCommand } from '../scripts/desktop-build.mjs';

test('bundleDirForArgs resolves default, target, and debug bundle directories', () => {
  const root = resolve('repo');
  const cargo = join(root, 'src-tauri', 'target');
  assert.equal(bundleDirForArgs([], root, {}), join(cargo, 'release', 'bundle'));
  assert.equal(bundleDirForArgs(['--debug'], root, {}), join(cargo, 'debug', 'bundle'));
  assert.equal(
    bundleDirForArgs(['--runner', 'cargo-xwin', '--target', 'x86_64-pc-windows-msvc', '--bundles', 'nsis'], root, {}),
    join(cargo, 'x86_64-pc-windows-msvc', 'release', 'bundle'),
  );
  assert.equal(
    bundleDirForArgs(['--target=x86_64-pc-windows-msvc', '-d'], root, {}),
    join(cargo, 'x86_64-pc-windows-msvc', 'debug', 'bundle'),
  );
  assert.equal(
    bundleDirForArgs([], root, { CARGO_TARGET_DIR: 'custom', CARGO_BUILD_TARGET: 'x86_64-pc-windows-msvc' }),
    join(root, 'src-tauri', 'custom', 'x86_64-pc-windows-msvc', 'release', 'bundle'),
  );
});

test('copies Linux and Windows installers from their build directories into separate artifacts folders', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'desktop-artifacts-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const [args, outputs] of [
    [[], [['deb', 'EZTYN AI MONITOR_1.0.0_amd64.deb'], ['rpm', 'monitor.rpm'], ['appimage', 'monitor.AppImage']]],
    [['--target', 'x86_64-pc-windows-msvc', '--debug'], [['nsis', 'monitor-setup.exe'], ['msi', 'monitor.msi']]],
  ] as const) {
    const bundle = bundleDirForArgs(args, root, {});
    const [kind, name] = outputs[0];
    const unpacked = join(bundle, kind, 'unpacked', name);
    mkdirSync(join(bundle, outputs[0][0], 'unpacked'), { recursive: true });
    writeFileSync(unpacked, 'not a final package');
    for (const [kind, name] of outputs) {
      mkdirSync(join(bundle, kind), { recursive: true });
      writeFileSync(join(bundle, kind, name), name);
    }
    writeFileSync(join(bundle, outputs[0][0], 'build.log'), 'not an installer');
    const exported = exportBundles(bundle, root);
    assert.equal(exported.length, outputs.length);
    for (const [kind, name] of outputs) {
      const destination = exported.find(file => basename(file) === name.replaceAll(' ', '_'));
      assert.ok(destination);
      assert.equal(readFileSync(destination, 'utf8'), name);
      assert.equal(readFileSync(join(bundle, kind, name), 'utf8'), name);
    }
    assert.equal(readFileSync(unpacked, 'utf8'), 'not a final package');
    assert.deepEqual(exportBundles(join(root, 'absent'), root), []);
  }
  assert.equal(readdirSync(join(root, 'artifacts')).length, 2);
});

test('refuses to copy installers through a linked artifacts directory', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'desktop-artifacts-link-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bundle = bundleDirForArgs([], root, {});
  const outside = join(root, 'other');
  mkdirSync(join(bundle, 'deb'), { recursive: true });
  mkdirSync(outside);
  symlinkSync(outside, join(root, 'artifacts'), 'junction');
  writeFileSync(join(bundle, 'deb', 'monitor.deb'), 'installer');
  assert.throws(() => exportBundles(bundle, root), /artifacts\/ must be a real directory/);
  assert.deepEqual(readdirSync(outside), []);
});

test('resolveTauriCommand prefers local @tauri-apps/cli via Node and falls back to tauri', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'desktop-cli-resolve-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  assert.deepEqual(resolveTauriCommand(root), { command: 'tauri', prefixArgs: [] });

  const localCli = join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
  mkdirSync(join(root, 'node_modules', '@tauri-apps', 'cli'), { recursive: true });
  writeFileSync(localCli, '// tauri cli stub\n');
  assert.deepEqual(resolveTauriCommand(root), { command: process.execPath, prefixArgs: [localCli] });
});

test('desktop-build invokes local @tauri-apps/cli without shell interpretation', (t) => {
  const root = mkdtempSync(join(tmpdir(), 'desktop-no-shell-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const localCli = join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
  mkdirSync(join(root, 'node_modules', '@tauri-apps', 'cli'), { recursive: true });
  writeFileSync(localCli, "require('node:fs').writeFileSync('argv.json', JSON.stringify(process.argv.slice(2)));\n");

  const wrapper = fileURLToPath(new URL('../scripts/desktop-build.mjs', import.meta.url));
  const run = spawnSync(process.execPath, [wrapper, '--no-bundle', '--', '--flag=$(touch pwned)&echo hi'], {
    cwd: root,
    encoding: 'utf8',
  });
  assert.ifError(run.error);
  assert.equal(run.status, 0, run.stderr);
  assert.deepEqual(
    JSON.parse(readFileSync(join(root, 'argv.json'), 'utf8')),
    ['build', '--features', 'desktop', '--no-bundle', '--', '--flag=$(touch pwned)&echo hi'],
  );
  assert.equal(existsSync(join(root, 'pwned')), false);
});

const config = fileURLToPath(new URL('../vite.config.ts', import.meta.url));
const vite = fileURLToPath(new URL('../node_modules/vite/bin/vite.js', import.meta.url));
const fixtureHtml = fileURLToPath(new URL('./fixtures/desktop-build/index.html', import.meta.url));
for (const mode of ['default', 'outside', 'linked-default', 'linked-override', 'alternate', 'invalid']) {
  test(`production build output: ${mode}`, (t) => {
    const base = mkdtempSync(join(tmpdir(), 'desktop-output-'));
    t.after(() => rmSync(base, { recursive: true, force: true }));
    const root = join(base, 'project');
    mkdirSync(root);
    copyFileSync(fixtureHtml, join(root, 'index.html'));
    writeFileSync(join(root, 'main.js'), 'console.log("fresh production bundle");');

    const linked = mode.startsWith('linked-');
    const usesDefault = mode === 'default' || mode === 'linked-default';
    const local = join(root, usesDefault ? 'dist' : 'custom-output');
    const out = linked || mode === 'outside' || mode === 'invalid'
      ? join(base, 'external') : local;
    mkdirSync(join(out, 'assets'), { recursive: true });
    if (linked) symlinkSync(out, local, 'junction');
    const stale = join(out, 'assets', 'retired-build.js');
    const bytes = Buffer.from('previous release asset\n');
    writeFileSync(stale, bytes);

    const args = ['build', '--config', mode === 'invalid' ? join(base, 'missing.ts') : config];
    if (!usesDefault) args.push('--outDir', linked ? local : out);
    const run = spawnSync(process.execPath, [vite, ...args], { cwd: root, encoding: 'utf8' });
    assert.ifError(run.error);

    if (mode === 'invalid') {
      assert.equal(run.status, 1);
      assert.match(run.stderr, /failed to load config/);
      assert.deepEqual(readFileSync(stale), bytes);
      assert.equal(existsSync(join(out, 'index.html')), false);
      return;
    }

    assert.equal(run.status, 0, run.stderr);
    if (mode === 'default') assert.equal(existsSync(stale), false);
    else assert.deepEqual(readFileSync(stale), bytes);
    if (mode === 'outside') {
      assert.doesNotMatch(run.stderr, /is not inside project root and will not be emptied/);
    }

    const html = readFileSync(join(out, 'index.html'), 'utf8');
    const bundle = html.match(/src="\/(assets\/[^"]+\.js)"/)?.[1];
    assert.ok(bundle);
    assert.match(readFileSync(join(out, bundle), 'utf8'), /fresh production bundle/);
  });
}
