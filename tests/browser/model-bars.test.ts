import test from 'node:test';
import assert from 'node:assert/strict';
test('approved offline history survives reload and has visible model bars under production CSP',async()=>{
 const cdp=process.env.WIDGET_CDP??'http://127.0.0.1:18458';
 const page=process.env.WIDGET_URL??'http://127.0.0.1:18457/';
 assert.equal(new URL(cdp).hostname,'127.0.0.1');assert.equal(new URL(page).hostname,'127.0.0.1');
 const targets=await(await fetch(`${cdp}/json/list`)).json();
 const ws=new WebSocket(targets.find((t:{type:string})=>t.type==='page').webSocketDebuggerUrl);
 await new Promise<void>(r=>{ws.onopen=()=>r();});let id=0;
 const pending=new Map<number,(v:any)=>void>();
 ws.onmessage=e=>{const r=JSON.parse(String(e.data));if(r.id){pending.get(r.id)?.(r);pending.delete(r.id);}};
 const call=(method:string,params={})=>new Promise<any>((resolve,reject)=>{pending.set(++id,r=>r.error?reject(r.error):resolve(r.result));ws.send(JSON.stringify({id,method,params}));});
 const inspect=async(expression:string)=>{const r=await call('Runtime.evaluate',{expression,returnByValue:true});assert.equal(r.exceptionDetails,undefined);return r.result.value;};
 let script:string|undefined;
 const click=async(selector:string)=>{const p=await inspect(`(()=>{const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2}})()`);await call('Input.dispatchMouseEvent',{type:'mousePressed',...p,button:'left',clickCount:1});await call('Input.dispatchMouseEvent',{type:'mouseReleased',...p,button:'left',clickCount:1});};
 try{
  await call('Page.enable');script=(await call('Page.addScriptToEvaluateOnNewDocument',{source:`globalThis.isTauri=true;window.calls=[];let allowed=true;const date=new Date().toISOString().slice(0,10),older=new Date(Date.now()-8*86400000).toISOString().slice(0,10);window.__TAURI_INTERNALS__={invoke:async(cmd,args)=>{window.calls.push(cmd);if(cmd==='usage_storage_directory')return '/synthetic/<local>/com.local.aiusagemonitor';if(cmd==='set_permission')allowed=args.allowed;return{agents:[{id:'claude',name:'Claude Code',kind:'cloud',installed:true,connected:false,monitoringAllowed:allowed,historyCached:true,running:null,status:'unavailable',message:'SYNTHETIC OFFLINE FIXTURE',models:[],usage:null,quotas:[],capabilities:[],updatedAt:new Date().toISOString(),usageHistory:[{date,inputTokens:12,outputTokens:6,totalTokens:18,cachedTokens:0,recordedResponses:2},{date:older,inputTokens:3,outputTokens:0,totalTokens:3,cachedTokens:0}],modelHistory:[{date,model:'alpha',totalTokens:12},{date:older,model:'alpha',totalTokens:3},{date,model:'beta',totalTokens:6}]}],scannedAt:'',platform:'Synthetic',warnings:[]};}};`})).identifier;
  for(const width of [480,390,320]){
   await call('Emulation.setDeviceMetricsOverride',{width,height:1800,deviceScaleFactor:1,mobile:false});await call('Page.navigate',{url:page});
   for(let n=0;n<100;n++){if(await inspect(`document.querySelectorAll('.tool-row').length===1`))break;await new Promise(r=>setTimeout(r,20));}
   await click('[data-page="usage"]');
   assert.equal(await inspect(`Boolean(document.querySelector('#usage-storage-button'))`),true);
   assert.equal(await inspect(`window.calls.includes('usage_storage_directory')`),false);
   await click('#usage-storage-button');
   assert.equal(await inspect(`document.querySelector('#usage-storage').open`),true);
   assert.equal(await inspect(`document.querySelector('#usage-storage-path').textContent`),'/synthetic/<local>/com.local.aiusagemonitor');
   assert.equal(await inspect(`document.querySelector('#usage-storage-path').childElementCount`),0);
   await click('#usage-storage button');
   assert.equal(await inspect(`document.querySelector('#usage-storage').open`),false);
   assert.equal(await inspect(`document.activeElement.id`),'usage-storage-button');
   assert.equal(await inspect(`document.querySelector('#usage-content .tool-select button').textContent`),'TOTAL USAGE');
   assert.equal(await inspect(`document.querySelector('[data-tool="total"]').getAttribute('aria-pressed')`),'true');
   await click('[data-period="90"]');
   assert.equal(await inspect(`document.querySelectorAll('.total-bars li').length`),90);
   assert.equal(await inspect(`document.documentElement.scrollWidth <= innerWidth`),true);
   assert.equal(await inspect(`document.querySelector('.total-bars li:not(.unrecorded) i').getBoundingClientRect().height > 0`),true);
   await click('#total-agents ~ .total-table button');
   assert.match(await inspect(`document.querySelector('#usage-content').textContent`),/Saved history/);
   assert.equal(await inspect(`window.calls.some(c=>c.startsWith('auth_'))`),false);
   for(const theme of ['light','dark']){
    if(await inspect(`document.documentElement.dataset.theme!==${JSON.stringify(theme)}`))await click('#theme');
    for(const period of [7,30]){
     await click(`[data-period="${period}"]`);
      const bars=await inspect(`[...document.querySelectorAll('#usage-content progress')].map(e=>({height:e.getBoundingClientRect().height,width:e.getBoundingClientRect().width,label:e.getAttribute('aria-label'),value:e.value,max:e.max,visible:e.closest('li').innerText}))`);
      const max=period===7?12:15;
      assert.deepEqual(bars.map((b:any)=>[b.label,b.value,b.max]),[['alpha',max,max],['beta',6,max]]);
      for(const b of bars){assert.ok(b.height>0&&b.width>0,`${width}/${theme}/${period}`);assert.ok(b.visible.includes(`${b.value} tokens`));}
    }
   }
  }
  await click('[data-page="context"]');
  await click('#context-content [data-action="revoke"]');
  await click('[data-page="usage"]');
  for(let n=0;n<100;n++){if(await inspect(`document.querySelector('#usage-content').textContent.includes('Permission required')`))break;await new Promise(r=>setTimeout(r,20));}
  assert.doesNotMatch(await inspect(`document.querySelector('#usage-content').textContent`),/alpha|beta|Saved history/);
  await click('[data-tool="total"]');
  assert.match(await inspect(`document.querySelector('#usage-content').textContent`),/No local usage recorded/);
 }finally{if(script)await call('Page.removeScriptToEvaluateOnNewDocument',{identifier:script});await call('Page.navigate',{url:page});ws.close();}
});
