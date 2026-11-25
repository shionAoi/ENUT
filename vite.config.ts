import { defineConfig } from 'vite';
import { lstatSync } from 'node:fs';
import { resolve } from 'node:path';

// https://vite.dev/config/server-options.html#server-strictport
export default defineConfig({
  server: { host: 'localhost', port: 1420, strictPort: true },
  clearScreen: false,
  build: { outDir: 'dist', target: 'es2022' },
  plugins: [{
    name: 'clean-default-build-output',
    config(config, { command }) {
      if (command !== 'build' || config.build?.emptyOutDir !== undefined) return;
      const root = config.root ?? process.cwd();
      const dist = resolve(root, 'dist');
      const outDir = resolve(root, config.build?.outDir ?? 'dist');
      // Keep custom output and linked directories out of automatic cleanup.
      const entry = outDir === dist ? lstatSync(dist, { throwIfNoEntry: false }) : undefined;
      return { build: { emptyOutDir: outDir === dist && (!entry || entry.isDirectory()) } };
    },
  }],
});
