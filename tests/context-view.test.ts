import test from 'node:test';
import assert from 'node:assert/strict';
import { renderContext } from '../src/context-view.ts';
import type { Agent } from '../src/types.ts';
const tool:Agent={id:'opencode',name:'OpenCode',kind:'cloud',installed:true,running:null,connected:true,status:'ready',message:'',models:[],usage:null,quotas:[],updatedAt:null,capabilities:[],context:{mcps:[{name:'<server>',status:'configured'},{name:'Live',status:'connected'},{name:'Auth',status:'needs_auth'}],skills:['<skill>'],note:'Skills partially unavailable'}};
test('Context escapes names and distinguishes configured from connected and partial failure',()=>{
  const html=renderContext([tool],'opencode',true,new Set(['opencode']));
  assert.match(html,/&lt;server&gt;/); assert.match(html,/Configured/); assert.match(html,/Connected/); assert.match(html,/Needs authentication/);
  assert.match(html,/&lt;skill&gt;/); assert.match(html,/Skills partially unavailable/); assert.doesNotMatch(html,/<skill>|<server>/);
});
test('Context hides cached data without permission or connection and preserves permission actions',()=>{
  for(const [connected,approved] of [[true,false],[false,true]] as const){
    const html=renderContext([{...tool,connected}],'opencode',true,new Set(approved?['opencode']:[]));
    assert.doesNotMatch(html,/&lt;server&gt;|Live|&lt;skill&gt;|partially/);
    assert.match(html,/unavailable|Permission required/i); assert.match(html,approved?/data-action="revoke"/:/data-action="connect"/);
  }
});
test('received empty Context and missing Context are different states',()=>{
  const empty=renderContext([{...tool,context:{mcps:[],skills:[],note:'Discovery completed'}}],'opencode',true,new Set(['opencode']));
  assert.match(empty,/No MCPs reported/); assert.match(empty,/No installed skills reported/); assert.match(empty,/Discovery completed/);
  assert.match(renderContext([{...tool,context:null}],'opencode',true,new Set(['opencode'])),/Context unavailable/);
  assert.match(renderContext([],null,false,new Set()),/desktop/i);
});
