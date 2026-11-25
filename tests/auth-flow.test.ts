import test from 'node:test';
import assert from 'node:assert/strict';
import { AuthFlow } from '../src/auth-flow.ts';
test('codes are submitted separately from choices and cancelled attempts cannot publish late state',async()=>{
 const calls:{command:string,args?:Record<string,unknown>}[]=[];
 let finish!:(value:unknown)=>void;
 const flow=new AuthFlow(async(command,args)=>{calls.push({command,args});if(command==='auth_methods')return[];if(command==='auth_start')return{attemptId:'a',toolId:'claude',state:'waiting',kind:'native',message:'',output:'',url:null,userCode:null};if(command==='auth_poll')return new Promise(r=>finish=r);return null;});
 await flow.load('claude');await flow.start('native');const poll=flow.poll();await flow.cancel();finish({attemptId:'a',toolId:'claude',state:'authenticated'});await poll;
 assert.equal(flow.view,null);assert.equal(calls.filter(c=>c.command==='auth_cancel').length,1);
 assert.ok(calls.every(c=>!JSON.stringify(c.args).includes('secret-code')));
});
test('device challenges reject paste-back codes and local runtimes do not start auth',async()=>{
 const calls:string[]=[];const flow=new AuthFlow(async(command)=>{calls.push(command);if(command==='auth_methods')return[];return{attemptId:'a',toolId:'codex',state:'waiting',kind:'device_code',message:'',output:'',url:'https://auth.openai.com/codex/device',userCode:'FIXTURE'};});
 await flow.load('codex');await flow.start('device');await assert.rejects(flow.submit('secret-code'));assert.ok(!calls.includes('auth_submit'));
 await flow.cancel();await flow.load('ollama');await assert.rejects(flow.start('native'));
});
test('closing during startup waits for and cancels the newly created attempt',async()=>{
 let resolve!:(value:unknown)=>void;const cancelled:string[]=[];
 const flow=new AuthFlow(async(command,args)=>{if(command==='auth_methods')return[];if(command==='auth_start')return new Promise(r=>resolve=r);if(command==='auth_cancel')cancelled.push(String(args?.attemptId));return null;});
 await flow.load('claude');const start=flow.start('native');const close=flow.cancel();
 resolve({attemptId:'late',toolId:'claude',state:'waiting',kind:'native'});await Promise.all([start,close]);
 assert.equal(flow.view,null);assert.deepEqual(cancelled,['late']);
});
test('failed cancellation retains only the attempt ID for a safe retry',async()=>{
 let calls=0;const flow=new AuthFlow(async(command)=>{if(command==='auth_methods')return[];if(command==='auth_start')return{attemptId:'retry',toolId:'claude',state:'waiting',kind:'native',output:'private instructions'};if(command==='auth_cancel'&&++calls===1)throw Error('offline');return null;});
 await flow.load('claude');await flow.start('native');await assert.rejects(flow.cancel());assert.equal(flow.view,null);await flow.cancel();assert.equal(calls,2);
});
