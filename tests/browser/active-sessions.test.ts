import test from 'node:test';
import assert from 'node:assert/strict';

// Dedicated synthetic localhost browser only; no real CLI or account access.
test('Usage never substitutes native sessions, quotas or OpenCode API totals for local model records',async()=>{
  const cdp=process.env.WIDGET_CDP??'http://127.0.0.1:18478';
  const page=process.env.WIDGET_URL??'http://127.0.0.1:18477/';
  assert.equal(new URL(cdp).hostname,'127.0.0.1');assert.equal(new URL(page).hostname,'127.0.0.1');
  const targets=await(await fetch(`${cdp}/json/list`)).json();
  const ws=new WebSocket(targets.find((t:{type:string})=>t.type==='page').webSocketDebuggerUrl);
  await new Promise<void>(r=>{ws.onopen=()=>r();});let id=0;
  const pending=new Map<number,(v:any)=>void>();
  ws.onmessage=e=>{const r=JSON.parse(String(e.data));if(r.id){pending.get(r.id)?.(r);pending.delete(r.id);}};
  const call=(method:string,params={})=>new Promise<any>((resolve,reject)=>{pending.set(++id,r=>r.error?reject(r.error):resolve(r.result));ws.send(JSON.stringify({id,method,params}));});
  const inspect=async(expression:string)=>{const r=await call('Runtime.evaluate',{expression,returnByValue:true});assert.equal(r.exceptionDetails,undefined);return r.result.value;};
  const click=async(selector:string)=>{
    const p=await inspect(`(()=>{const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2}})()`);
    await call('Input.dispatchMouseEvent',{type:'mousePressed',...p,button:'left',clickCount:1});
    await call('Input.dispatchMouseEvent',{type:'mouseReleased',...p,button:'left',clickCount:1});
  };
  const until=async(expression:string)=>{for(let n=0;n<100;n++){if(await inspect(expression))return;await new Promise(r=>setTimeout(r,30));}assert.fail(expression);};
  let script:string|undefined;
  try {
    await call('Page.enable');
    for(const mode of ['running','zero','unknown','stale']) {
      if(script)await call('Page.removeScriptToEvaluateOnNewDocument',{identifier:script});
      script=(await call('Page.addScriptToEvaluateOnNewDocument',{source:`
        globalThis.isTauri=true;let allowed=true;const mode=${JSON.stringify(mode)};
        window.__TAURI_INTERNALS__={invoke:async(cmd,args)=>{
          if(cmd==='set_permission')allowed=args.allowed;
          return {agents:[{id:'opencode',name:'OpenCode',kind:'cloud',installed:true,running:null,connected:allowed,status:'ready',message:'',models:[],usage:null,modelUsage:[{model:'Remote model',totalTokens:321}],quotas:[{id:'fixture',label:'Fixture quota',remainingPercent:42,resetsAt:null,source:'Synthetic'}],capabilities:[],updatedAt:null,activeSessions:mode==='unknown'?null:{sessions:mode==='zero'?[]:[{id:'ses_fixture',state:'running'}],source:'Synthetic native source',scope:'Synthetic service only',asOf:new Date(Date.now()-(mode==='stale'?180000:0)).toISOString()}}],scannedAt:'',platform:'Synthetic',warnings:[]};
        }};`})).identifier;
      await call('Page.navigate',{url:page});await until(`document.querySelectorAll('.tool-row').length===1`);
      assert.match(await inspect(`document.querySelector('#agents').textContent`),/42% left/);
      await click('[data-page="usage"]');
      await click('[data-section="usage"][data-tool="opencode"]');
      const text=await inspect(`document.querySelector('#usage-content').textContent`);
      assert.doesNotMatch(text,/Fixture quota|42%|Quota & resets/);
      assert.match(text,/No local model usage/);
      assert.doesNotMatch(text,/Active sessions|ses_fixture|Synthetic native source|Synthetic service only|\d+ running|Remote model|321/);
      if(mode==='running') {
        await click('[data-page="context"]');await click('#context-content [data-action="revoke"]');
        await click('[data-page="usage"]');await until(`document.querySelector('#usage-content').textContent.includes('Permission required')`);
        assert.doesNotMatch(await inspect(`document.querySelector('#usage-content').textContent`),/Synthetic native source|Synthetic service only/);
      }
    }
  } finally {if(script)await call('Page.removeScriptToEvaluateOnNewDocument',{identifier:script});ws.close();}
});
