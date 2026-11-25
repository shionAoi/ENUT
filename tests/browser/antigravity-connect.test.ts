import test from 'node:test';
import assert from 'node:assert/strict';

// Run only against a dedicated localhost test browser, never a personal profile.
test('Gemini Connect opens owned code-entry login; completion and restart preserve consent boundaries',async()=>{
 const cdp=process.env.WIDGET_CDP??'http://127.0.0.1:18478';
 const page=process.env.WIDGET_URL??'http://127.0.0.1:18477/';
 assert.equal(new URL(cdp).hostname,'127.0.0.1');assert.equal(new URL(page).hostname,'127.0.0.1');
 const targets=await(await fetch(`${cdp}/json/list`)).json();
 const ws=new WebSocket(targets.find((t:{type:string})=>t.type==='page').webSocketDebuggerUrl);
 await new Promise<void>(r=>{ws.onopen=()=>r();});let id=0;
 const pending=new Map<number,(r:any)=>void>();
 ws.onmessage=e=>{const r=JSON.parse(String(e.data));if(r.id){pending.get(r.id)?.(r);pending.delete(r.id);}};
 const call=(method:string,params={})=>new Promise<any>((resolve,reject)=>{
  pending.set(++id,r=>r.error?reject(r.error):resolve(r.result));ws.send(JSON.stringify({id,method,params}));
 });
 const inspect=async(expression:string)=>{
  const r=await call('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true});
  assert.equal(r.exceptionDetails,undefined);return r.result.value;
 };
 async function until(expression:string){
  for(let n=0;n<100;n++){if(await inspect(expression))return;await new Promise(r=>setTimeout(r,30));}
  assert.fail(`Timed out: ${expression}`);
 }
 const fixture=`(()=>{
  window.isTauri=true;window.calls=[];window.state='waiting';window.allowed={};
  const agents=['antigravity','codex','ollama'].map(id=>({id,name:id,kind:id==='ollama'?'local':'cloud',installed:true,running:false,connected:false,status:'detected',message:'Synthetic fixture',models:[],usage:null,quotas:[],updatedAt:null,capabilities:[]}));
   const snapshot=()=>({agents:agents.map(a=>({...a,monitoringAllowed:!!window.allowed[a.id],authentication:window.required&&a.id===window.requiredTool?'required':'unknown',connected:!!window.allowed[a.id]})),scannedAt:new Date().toISOString(),platform:'synthetic',warnings:[]});
   const view=()=>({attemptId:'synthetic-attempt',toolId:window.requiredTool,state:window.state,kind:window.requiredTool==='codex'?'device_code':'native',message:'Synthetic waiting',output:'Synthetic instructions',url:null,userCode:window.requiredTool==='codex'?'SYNTHETIC-DEVICE':null});
  window.__TAURI_INTERNALS__={invoke:async(cmd,args)=>{
   window.calls.push({cmd,args});
   switch(cmd){
    case 'scan_agents':case 'refresh_usage':return snapshot();
    case 'set_permission':window.allowed[args.agentId]=args.allowed;return snapshot();
     case 'auth_methods':if(window.holdMethods)await new Promise(r=>window.releaseMethods=r);return args.agentId==='codex'?[{id:'chatgpt-device',provider:'chatgpt',method:'device',kind:'device_code',label:'ChatGPT device code'}]:[{id:'fixed-native',provider:'google',method:'native',kind:'native',label:'Native'}];
    case 'auth_start':window.allowed[args.agentId]=false;if(window.holdStart)await new Promise(r=>window.releaseStart=r);return view();
    case 'auth_poll':if(window.holdPoll)await new Promise(r=>window.releasePoll=r);return view();
    case 'auth_submit':case 'auth_cancel':return null;
    default:throw Error('Unexpected synthetic command');
   }
  }};
 })();`;
 let script:string|undefined;
  async function reset(required=true,tool='antigravity'){
  if(script)await call('Page.removeScriptToEvaluateOnNewDocument',{identifier:script});
   script=(await call('Page.addScriptToEvaluateOnNewDocument',{source:fixture+`;window.required=${required};window.requiredTool=${JSON.stringify(tool)};`})).identifier;
  await call('Page.navigate',{url:page});
   await until(`document.querySelector('[data-agent="${tool}"][data-action="${required?'login':'connect'}"]')&&window.calls.filter(c=>c.cmd==='refresh_usage').length===1`);
 }
 async function connect(){
   assert.equal(await inspect(`document.querySelectorAll('[data-agent="antigravity"][data-action="connect"],[data-auth-tool="antigravity"]').length`),0);
   await inspect(`document.querySelector('[data-agent="antigravity"][data-action="login"]').click()`);
  assert.match(await inspect(`document.querySelector('#permission-details').textContent`),/may open a browser and change the tool’s active account/);
   assert.equal(await inspect(`document.querySelector('#allow').textContent`),'Allow native login');
  await inspect(`document.querySelector('#allow').click()`);await until(`document.querySelector('#native-auth').open`);
 }
 const grants=()=>inspect(`window.calls.filter(c=>c.cmd==='set_permission'&&c.args.allowed).length`);
 async function escape(){
  await call('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
  await call('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
 }
 try{
   await call('Page.enable');await reset(false);
   assert.equal(await inspect(`window.calls.filter(c=>c.cmd.startsWith('auth_')).length`),0);
   await inspect(`document.querySelector('[data-agent="antigravity"][data-action="connect"]').click();document.querySelector('#allow').click()`);
   await until(`window.calls.some(c=>c.cmd==='auth_poll')`);
   assert.equal(await grants(),0);
   assert.equal(await inspect(`document.querySelector('#auth-paste').hidden`),false);
   assert.equal(await inspect(`window.calls.filter(c=>c.cmd==='set_permission'&&!c.args.allowed).length`),0);
   await escape();await until(`!document.querySelector('#native-auth').open`);
   await reset();await connect();await until(`window.calls.some(c=>c.cmd==='auth_poll')`);
  assert.equal(await grants(),0);assert.equal(await inspect(`document.querySelector('#auth-paste').hidden`),false);
  assert.equal(await inspect(`document.querySelector('#auth-code').type`),'password');
  assert.deepEqual(await inspect(`window.calls.find(c=>c.cmd==='auth_start').args`),{agentId:'antigravity',choiceId:'fixed-native'});
  await inspect(`document.querySelector('#auth-code').value='SYNTHETIC_CODE_123';document.querySelector('#auth-code-form').requestSubmit()`);
  assert.equal(await inspect(`document.querySelector('#auth-code').value`),'');
  assert.deepEqual(await inspect(`window.calls.find(c=>c.cmd==='auth_submit').args`),{attemptId:'synthetic-attempt',code:'SYNTHETIC_CODE_123'});
  assert.equal(await inspect(`document.body.textContent.includes('SYNTHETIC_CODE_123')`),false);
  assert.equal(await inspect(`window.calls.filter(c=>c.cmd==='refresh_usage').length`),1);
  await inspect(`window.state='finished'`);await until(`document.querySelector('#auth-close').textContent==='Finish and connect'`);
  assert.equal(await grants(),0);await inspect(`document.querySelector('#auth-close').click()`);await until(`window.allowed.antigravity===true`);
  const commands=await inspect(`window.calls.map(c=>c.cmd+(c.cmd==='set_permission'?':'+c.args.allowed:''))`);
  const cancel=commands.indexOf('auth_cancel');assert.ok(cancel>commands.indexOf('auth_submit'));
  assert.deepEqual(commands.slice(cancel,cancel+4),['auth_cancel','scan_agents','set_permission:true','refresh_usage']);
  for(const state of ['failed','expired','cancelled']){
   await reset();await inspect(`window.state=${JSON.stringify(state)}`);await connect();
   await until(`document.querySelector('#auth-close').textContent==='Finish / close'`);
   await inspect(`document.querySelector('#auth-close').click()`);await until(`!document.querySelector('#native-auth').open`);assert.equal(await grants(),0);
  }
  for(const terminal of [false,true]){
   await reset();await connect();await until(`window.calls.some(c=>c.cmd==='auth_poll')`);
   if(terminal){await inspect(`window.state='finished'`);await until(`document.querySelector('#auth-close').textContent==='Finish and connect'`);}
   await escape();await until(`!document.querySelector('#native-auth').open`);assert.equal(await grants(),0);
  }
  for(const hold of ['Methods','Start','Poll']){
   await reset();await inspect(`window.hold${hold}=true`);await connect();await until(`window.release${hold}!==undefined`);
   await escape();await inspect(`window.release${hold}()`);await until(`!document.querySelector('#native-auth').open`);assert.equal(await grants(),0);
  }
   for(const tool of ['codex','ollama']){
   await reset();await inspect(`document.querySelector('[data-agent="${tool}"][data-action="connect"]').click();document.querySelector('#allow').click()`);
   await until(`window.allowed.${tool}===true`);assert.equal(await inspect(`window.calls.filter(c=>c.cmd.startsWith('auth_')).length`),0);
   }
   await reset(true,'codex');
   await inspect(`document.querySelector('[data-agent="codex"][data-action="login"]').click();document.querySelector('#allow').click()`);
   await until(`window.calls.some(c=>c.cmd==='auth_poll')`);
   assert.equal(await grants(),0);
   assert.equal(await inspect(`document.querySelector('#auth-paste').hidden`),true);
   assert.deepEqual(await inspect(`window.calls.find(c=>c.cmd==='auth_start').args`),{agentId:'codex',choiceId:'chatgpt-device'});
   await inspect(`window.state='authenticated'`);await until(`document.querySelector('#auth-close').textContent==='Finish and connect'`);
   assert.equal(await grants(),0);await inspect(`document.querySelector('#auth-close').click()`);await until(`window.allowed.codex===true`);
 }finally{
  if(script)await call('Page.removeScriptToEvaluateOnNewDocument',{identifier:script});
  ws.close();
 }
});
