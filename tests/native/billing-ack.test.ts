import test from 'node:test';
import assert from 'node:assert/strict';
import { cpSync, readFileSync, writeFileSync, mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir, homedir } from 'node:os';
import { resolve, join } from 'node:path';
import { spawnSync } from 'node:child_process';

test('billing connection acknowledges permission without blocking on provider reads', t => {
  const root = resolve(process.env.WIDGET_SOURCE_ROOT ?? '.');
  const scratch = mkdtempSync(join(tmpdir(), 'billing-ack-'));
  console.log(`Temporary scratch: ${scratch}`);
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  mkdirSync(join(scratch, 'src-tauri/tests'), { recursive: true });
  for (const name of ['src', 'Cargo.toml', 'Cargo.lock', 'build.rs']) {
    cpSync(join(root, 'src-tauri', name), join(scratch, 'src-tauri', name), { recursive: true });
  }
  const source = readFileSync(join(root, 'src-tauri/src/main.rs'), 'utf8');
  const command = source.match(/async fn connect_billing\([\s\S]*?\n\}/)?.[0];
  const scan = source.match(/fn scan_and_publish\([\s\S]*?\n\}/)?.[0];
  assert.ok(command, 'The production connection command must be tested');
  assert.ok(scan, 'The production scan helper must be tested');
  writeFileSync(join(scratch, 'src-tauri/tests/billing_ack.rs'), `
use ai_usage_monitor::{Discovery, StatusReader, Observation, Service, Snapshot};
use ai_usage_monitor::billing::{BillingStore, ReportReader, ReportFuture};
struct D;
impl Discovery for D { fn installed(&self, id: &str) -> bool { id == "codex" } }
struct R;
impl StatusReader for R {
    fn read(&self, _: String) -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<Observation,String>> + Send>> {
        Box::pin(async { Ok(Observation::default()) })
    }
}
type Backend = Service<D, R>;
type State<'a, T> = &'a T;
struct App;
fn publish_panel(_: &App) -> Result<(), String> { Ok(()) }
struct ReportingClient;
impl ReportReader for ReportingClient {
    fn read(&self, _: String, _: zeroize::Zeroizing<String>) -> ReportFuture {
        Box::pin(std::future::pending())
    }
}
fn decorate(mut snapshot: Snapshot, billing: &BillingStore<ReportingClient>) -> Snapshot {
    snapshot.billing = billing.reports(); snapshot
}
${[scan, command].join('\n').replaceAll('tauri::AppHandle', 'App')}
#[tokio::test]
async fn connection_returns_before_an_unresponsive_provider() {
    let backend = Service::new(D, R);
    let billing = BillingStore::new(ReportingClient);
    let result = tokio::time::timeout(std::time::Duration::from_millis(100),
        connect_billing(&backend, &billing, "openai".into(), "synthetic-key-not-a-secret".into(), App)).await;
    let snapshot = result.expect("Permission acknowledgement waited for the provider").unwrap();
    assert_eq!(snapshot.billing.len(), 1);
    assert!(snapshot.billing[0].spend_usd.is_none());
    billing.disconnect("openai").unwrap();
    assert!(billing.reports().is_empty());
}
`);
  mkdirSync(join(scratch, 'home'));
  mkdirSync(join(scratch, 'tmp'));
  const result = spawnSync('cargo', ['test', '--offline', '--manifest-path', join(scratch, 'src-tauri/Cargo.toml'), '--no-default-features', '--test', 'billing_ack'], {
    encoding: 'utf8', env: { ...process.env, HOME: join(scratch, 'home'), TMPDIR: join(scratch, 'tmp'), RUSTUP_HOME: process.env.RUSTUP_HOME ?? join(homedir(), '.rustup'), CARGO_HOME: process.env.CARGO_HOME ?? join(homedir(), '.cargo'), CARGO_TARGET_DIR: process.env.WIDGET_CARGO_TARGET_DIR ?? join(scratch, 'target') },
  });
  assert.equal(result.status, 0, result.stdout + result.stderr);
});
