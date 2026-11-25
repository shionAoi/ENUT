import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {AuthFlow} from '../src/auth-flow.ts';

test('native paste-back instructions are visible without expanding a disclosure',()=>{
 const source=readFileSync(new URL('../src/auth-ui.ts',import.meta.url),'utf8');
 assert.match(source,/<section id="auth-paste"/);
 assert.match(source,/Return here after browser sign-in/);
 assert.match(source,/Never paste a password, API key or access token/);
 assert.match(source,/autocapitalize="none"/);
});

test('required Login offers only exact native choices, while main has no competing sign-in injection',()=>{
 const ui=readFileSync(new URL('../src/auth-ui.ts',import.meta.url),'utf8');
 const main=readFileSync(new URL('../src/main.ts',import.meta.url),'utf8');
 assert.match(ui,/c\.provider==='chatgpt'&&c\.method==='device'&&c\.kind==='device_code'/);
 assert.doesNotMatch(main,/signIn\.dataset\.authTool/);
 assert.match(main,/pendingLogin/);
});

test('Antigravity paste-back forwards only to its active native attempt',async()=>{
 const submitted:unknown[]=[];
 const flow=new AuthFlow(async(command,args)=>{
  if(command==='auth_methods')return[];
  if(command==='auth_start')return{attemptId:'synthetic-agy',toolId:'antigravity',kind:'native',state:'waiting',message:'',output:'',url:null,userCode:null};
  if(command==='auth_submit')submitted.push(args);
  return null;
 });
 await flow.load('antigravity');await flow.start('google-native');
 await flow.submit('synthetic-authorization-code');
 assert.deepEqual(submitted,[{attemptId:'synthetic-agy',code:'synthetic-authorization-code'}]);
 assert.equal(flow.view?.state,'waiting');
 await flow.cancel();await assert.rejects(flow.submit('synthetic-authorization-code'));
 assert.equal(submitted.length,1);
});
