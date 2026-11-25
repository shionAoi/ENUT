import test from 'node:test';
import assert from 'node:assert/strict';

test('Toolbox title and controls remain separate at widget widths', async () => {
  const cdp=process.env.WIDGET_CDP??'http://127.0.0.1:18348';
  const page=process.env.WIDGET_URL??'http://127.0.0.1:18347/';
  assert.equal(new URL(cdp).hostname,'127.0.0.1');
  assert.equal(new URL(page).hostname,'127.0.0.1');
  const targets=await(await fetch(`${cdp}/json/list`)).json();
  const ws=new WebSocket(targets.find((target:{type:string})=>target.type==='page').webSocketDebuggerUrl);
  await new Promise<void>(resolve=>{ws.onopen=()=>resolve();});
  let id=0;
  const pending=new Map<number,(result:any)=>void>();
  ws.onmessage=event=>{const reply=JSON.parse(String(event.data));if(reply.id){pending.get(reply.id)?.(reply);pending.delete(reply.id);}};
  const call=(method:string,params={})=>new Promise<any>((resolve,reject)=>{pending.set(++id,reply=>reply.error?reject(reply.error):resolve(reply.result));ws.send(JSON.stringify({id,method,params}));});
  try {
    for(const width of [320,390,480,1280]) {
      await call('Emulation.setDeviceMetricsOverride',{width,height:900,deviceScaleFactor:1,mobile:false});
      await call('Page.navigate',{url:page});
      await new Promise(resolve=>setTimeout(resolve,300));
      const result=await call('Runtime.evaluate',{returnByValue:true,expression:`(()=>{const title=document.querySelector('h1').getBoundingClientRect();const actions=document.querySelector('.header-actions').getBoundingClientRect();return{intersects:title.left<actions.right&&actions.left<title.right&&title.top<actions.bottom&&actions.top<title.bottom,titleHeight:title.height,lineHeight:parseFloat(getComputedStyle(document.querySelector('h1')).lineHeight),overflow:document.documentElement.scrollWidth>innerWidth};})()`});
      const actual=result.result.value;
      assert.equal(actual.intersects,false,`${width}px title overlaps controls: ${JSON.stringify(actual)}`);
      assert.ok(actual.titleHeight<=actual.lineHeight*2,`${width}px title exceeds two lines`);
      assert.equal(actual.overflow,false);
    }
  } finally {ws.close();}
});
