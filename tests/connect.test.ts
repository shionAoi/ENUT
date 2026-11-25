import test from 'node:test';
import assert from 'node:assert/strict';
import {connectIntegration} from '../src/connect.ts';

test('Gemini Connect owns native login and grants only after explicit completion',async()=>{
 let grants=0, opens=0;
 let finish!:()=>Promise<void>;
 await connectIntegration('antigravity',async(tool,onFinished)=>{assert.equal(tool,'antigravity');opens++;finish=onFinished;},async()=>{grants++;});
 assert.equal(opens,1);
 assert.equal(grants,0);
 await finish();
 assert.equal(grants,1);
});
test('cancelled or failed Gemini login does not grant monitoring',async()=>{
 let grants=0;
 await assert.rejects(connectIntegration('antigravity',async()=>{throw Error('cancelled');},async()=>{grants++;}));
 assert.equal(grants,0);
});
test('other integrations retain their existing Connect permission behavior',async()=>{
 for(const tool of ['codex','claude','gemini','opencode','hermes','ollama','lmstudio'] as const){
  let grants=0;
  await connectIntegration(tool,async()=>{assert.fail('must not start native login');},async()=>{grants++;});
  assert.equal(grants,1,tool);
 }
});
