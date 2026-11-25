import test from 'node:test';
import assert from 'node:assert/strict';
import {cpSync,readFileSync,writeFileSync,mkdtempSync,mkdirSync,rmSync} from 'node:fs';
import {tmpdir,homedir} from 'node:os';
import {resolve,join} from 'node:path';
import {spawnSync} from 'node:child_process';

test('native tray actions survive refresh and stale quota cannot be republished',t=>{
  const root=resolve(process.env.WIDGET_SOURCE_ROOT??'.');
  const scratch=mkdtempSync(join(tmpdir(),'panel-regression-'));
  console.log(`Temporary scratch: ${scratch}`);
  t.after(()=>rmSync(scratch,{recursive:true,force:true}));
  mkdirSync(join(scratch,'src-tauri/tests'),{recursive:true});
  for(const name of ['src','Cargo.toml','Cargo.lock','build.rs']){
    cpSync(join(root,'src-tauri',name),join(scratch,'src-tauri',name),{recursive:true});
  }
  const source=readFileSync(join(root,'src-tauri/src/main.rs'),'utf8');
  const start=source.indexOf('fn publish_panel(');
  const end=source.indexOf('\n#[tauri::command]',start);
  assert.ok(start>=0&&end>start);
  const command=(name:string)=>{
    const start=source.indexOf(`async fn ${name}(`);
    return source.slice(start,source.indexOf('\n}',start)+2);
  };
  const production=[source.slice(start,end),command('set_permission'),command('auth_start')]
    .join('\n').replaceAll('tauri::AppHandle','App');
  const template=readFileSync(new URL('./panel-harness.rs.txt',import.meta.url),'utf8');
  writeFileSync(join(scratch,'src-tauri/tests/panel_verifier.rs'),template.replace('// INSERT PRODUCTION FUNCTIONS',()=>production));
  mkdirSync(join(scratch,'home'));mkdirSync(join(scratch,'tmp'));
  const result=spawnSync('cargo',['test','--offline','--manifest-path',join(scratch,'src-tauri/Cargo.toml'),'--no-default-features','--test','panel_verifier','--','--nocapture'],{encoding:'utf8',env:{...process.env,HOME:join(scratch,'home'),TMPDIR:join(scratch,'tmp'),RUSTUP_HOME:process.env.RUSTUP_HOME??join(homedir(),'.rustup'),CARGO_HOME:process.env.CARGO_HOME??join(homedir(),'.cargo'),CARGO_TARGET_DIR:process.env.WIDGET_CARGO_TARGET_DIR??join(scratch,'target')}});
  assert.equal(result.status,0,result.stdout+result.stderr);
});
