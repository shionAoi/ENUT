import { constants, copyFileSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

export function bundleDirForArgs(args, cwd = process.cwd(), env = process.env) {
  let target = env.CARGO_BUILD_TARGET;
  let debug = false;
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === '--debug' || arg === '-d') debug = true;
    else if ((arg === '--target' || arg === '-t') && args[i + 1]) target = args[++i];
    else if (arg.startsWith('--target=') || arg.startsWith('-t=')) target = arg.split('=')[1];
  }
  const profile = debug ? 'debug' : 'release';
  const cargoDir = resolve(cwd, 'src-tauri', env.CARGO_TARGET_DIR || 'target');
  return join(cargoDir, target || '', profile, 'bundle');
}

const installerSuffixes = { deb: '.deb', rpm: '.rpm', appimage: '.appimage', nsis: '.exe', msi: '.msi' };

export function exportBundles(bundleDir, cwd = process.cwd()) {
  const files = [];
  for (const [kind, suffix] of Object.entries(installerSuffixes)) {
    const dir = join(bundleDir, kind);
    if (!lstatSync(dir, { throwIfNoEntry: false })?.isDirectory()) continue;
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (!entry.isFile() || !entry.name.toLowerCase().endsWith(suffix)) continue;
      files.push(join(dir, entry.name));
    }
  }
  if (!files.length) return [];
  const artifacts = resolve(cwd, 'artifacts');
  mkdirSync(artifacts, { recursive: true });
  if (!lstatSync(artifacts).isDirectory()) throw new Error('artifacts/ must be a real directory');
  const output = mkdtempSync(join(artifacts, 'desktop-'));
  return files.map(file => {
    const destination = join(output, basename(file).replaceAll(' ', '_'));
    copyFileSync(file, destination, constants.COPYFILE_EXCL);
    return destination;
  });
}

export function resolveTauriCommand(cwd = process.cwd()) {
  const localCli = resolve(cwd, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
  if (statSync(localCli, { throwIfNoEntry: false })?.isFile()) {
    return { command: process.execPath, prefixArgs: [localCli] };
  }
  return { command: 'tauri', prefixArgs: [] };
}

function build(args) {
  const separator = args.indexOf('--');
  const tauriArgs = separator === -1 ? args : args.slice(0, separator);
  const cargoArgs = separator === -1 ? [] : args.slice(separator + 1);
  const bundle = !tauriArgs.includes('--no-bundle');
  if (bundle && cargoArgs.some(arg => /^--target-dir(?:=|$)/.test(arg))) {
    throw new Error('Installer builds use an isolated Cargo target directory; remove --target-dir.');
  }
  // A shared Cargo target cannot identify which invocation produced an installer.
  const targetDir = bundle ? mkdtempSync(join(tmpdir(), 'eztyn-desktop-target-')) : null;
  const env = targetDir ? { ...process.env, CARGO_TARGET_DIR: targetDir } : process.env;
  const { command, prefixArgs } = resolveTauriCommand(process.cwd());
  try {
    const result = spawnSync(command, [...prefixArgs, 'build', '--features', 'desktop', ...args], {
      env,
      stdio: 'inherit',
      shell: false,
    });
    if (result.error) throw result.error;
    const status = result.status ?? 1;
    if (targetDir && status === 0) {
      for (const file of exportBundles(bundleDirForArgs(tauriArgs, process.cwd(), env))) console.log(`Installer: ${file}`);
    }
    return status;
  } finally {
    if (targetDir) rmSync(targetDir, { recursive: true, force: true, maxRetries: 3 });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    process.exitCode = build(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
