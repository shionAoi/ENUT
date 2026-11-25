import test from 'node:test';
import assert from 'node:assert/strict';

test('hover and keyboard focus show only the connection tooltip without expanding the quota row', async () => {
  const cdp = process.env.WIDGET_CDP ?? 'http://127.0.0.1:18458';
  const page = process.env.WIDGET_URL ?? 'http://127.0.0.1:18457/';
  assert.equal(new URL(cdp).hostname, '127.0.0.1');
  assert.equal(new URL(page).hostname, '127.0.0.1');
  const targets = await (await fetch(`${cdp}/json/list`)).json();
  const ws = new WebSocket(targets.find((t: {type: string}) => t.type === 'page').webSocketDebuggerUrl);
  await new Promise<void>(resolve => { ws.onopen = () => resolve(); });
  let id = 0;
  const pending = new Map<number, (reply: any) => void>();
  ws.onmessage = event => { const reply = JSON.parse(String(event.data)); pending.get(reply.id)?.(reply); pending.delete(reply.id); };
  const call = (method: string, params = {}) => new Promise<any>((resolve, reject) => {
    pending.set(++id, reply => reply.error ? reject(reply.error) : resolve(reply.result));
    ws.send(JSON.stringify({id, method, params}));
  });
  const inspect = async (expression: string) => {
    const reply = await call('Runtime.evaluate', {expression, returnByValue: true});
    assert.equal(reply.exceptionDetails, undefined);
    return reply.result.value;
  };
  const box = (selector: string) => inspect(`document.querySelector(${JSON.stringify(selector)})?.getBoundingClientRect().toJSON()`);
  const move = (x: number, y: number) => call('Input.dispatchMouseEvent', {type: 'mouseMoved', x, y});
  const click = async ({x, y, width, height}: {x: number; y: number; width: number; height: number}) => {
    const params = {x: x + width / 2, y: y + height / 2, button: 'left', clickCount: 1};
    await call('Input.dispatchMouseEvent', {type: 'mousePressed', ...params});
    await call('Input.dispatchMouseEvent', {type: 'mouseReleased', ...params});
  };
  const visible = () => inspect(`document.querySelector('[role="tooltip"]')?.getBoundingClientRect().height > 0`);
  let script: string | undefined;
  try {
    await call('Page.enable');
    script = (await call('Page.addScriptToEvaluateOnNewDocument', {source: `
      globalThis.isTauri = true;
      window.__TAURI_INTERNALS__ = {invoke: async () => ({
        agents: [{id:'codex',name:'Codex',kind:'cloud',installed:true,connected:true,monitoringAllowed:true,
          running:true,status:'ready',message:'Synthetic fixture',models:[],usage:null,
          quotas:[{id:'session',label:'Session',remainingPercent:62,resetsAt:'2099-11-07T15:08:00Z',source:'Fixture provider'}],capabilities:[],updatedAt:null}],
        scannedAt:'',platform:'Synthetic',warnings:[]
      })};
    `})).identifier;
    await call('Emulation.setDeviceMetricsOverride', {width: 320, height: 900, deviceScaleFactor: 1, mobile: false});
    await call('Page.navigate', {url: page});
    for (let n = 0; n < 100 && !await box('.quota-help'); n++) await new Promise(resolve => setTimeout(resolve, 20));
    const trigger = await box('.quota-help');
    assert.ok(trigger, 'help trigger rendered');
    const row = await box('.tool-row');
    await move(0, 0);
    assert.equal(await visible(), false);
    await move(trigger.x + trigger.width / 2, trigger.y + trigger.height / 2);
    assert.equal(await visible(), true, 'hover shows a floating tooltip');
    assert.match(await inspect(`document.querySelector('[role="tooltip"]').textContent`), /Revoke removes/);
    assert.equal((await box('.tool-row')).height, row.height);
    assert.equal(await inspect(`document.querySelector('#detail-codex').hidden`), true);
    const tip = await box('[role="tooltip"]');
    assert.ok(tip.left >= 0 && tip.right <= 320);
    await move(tip.x + tip.width / 2, tip.y + tip.height / 2);
    assert.equal(await visible(), true, 'the tooltip stays open while reading with the pointer');
    await move(0, 0);
    assert.equal(await visible(), false);

    for (let n = 0; n < 12 && !(await inspect(`document.activeElement?.matches('.quota-help')`)); n++) {
      await call('Input.dispatchKeyEvent', {type: 'keyDown', key: 'Tab', windowsVirtualKeyCode: 9});
      await call('Input.dispatchKeyEvent', {type: 'keyUp', key: 'Tab', windowsVirtualKeyCode: 9});
    }
    assert.equal(await inspect(`document.activeElement?.matches('.quota-help')`), true);
    assert.equal(await visible(), true);
    await call('Input.dispatchKeyEvent', {type: 'keyDown', key: 'Escape', windowsVirtualKeyCode: 27});
    await call('Input.dispatchKeyEvent', {type: 'keyUp', key: 'Escape', windowsVirtualKeyCode: 27});
    assert.equal(await visible(), false, 'Escape dismisses without moving focus');
    assert.equal(await inspect(`document.activeElement?.matches('.quota-help')`), true);
    await move(trigger.x + trigger.width / 2, trigger.y + trigger.height / 2);
    await click(trigger);
    assert.equal(await inspect(`document.querySelector('#detail-codex').hidden`), true, 'clicking help does not expand the row');
    await move(0, 0);
    await call('Input.dispatchKeyEvent', {type: 'keyDown', key: 'Tab', windowsVirtualKeyCode: 9});
    await call('Input.dispatchKeyEvent', {type: 'keyUp', key: 'Tab', windowsVirtualKeyCode: 9});
    assert.equal(await visible(), false);
    await click(await box('.quota-details-toggle'));
    assert.equal(await inspect(`document.querySelector('#detail-codex').hidden`), false);
    assert.match(await inspect(`document.querySelector('#detail-codex').innerText`), /62% remaining[\s\S]*Source: Fixture provider/);
    const ring = await box('.quota-ring');
    assert.ok(ring.width > 0 && ring.width === ring.height && ring.right < 320);
    assert.match(await inspect(`getComputedStyle(document.querySelector('.quota-ring'), '::before').backgroundImage`), /conic-gradient\(.+62%/);
    assert.equal(await inspect(`document.querySelector('#detail-codex time').dateTime`), '2099-11-07T15:08:00Z');
    assert.doesNotMatch(await inspect(`document.querySelector('#detail-codex').innerText`), /Synthetic fixture|remembered monitoring access/);
  } finally {
    if (script) await call('Page.removeScriptToEvaluateOnNewDocument', {identifier: script});
    await call('Page.navigate', {url: page});
    ws.close();
  }
});
