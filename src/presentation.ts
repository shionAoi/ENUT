import type {Agent,Quota} from './types.ts';

function compactWindow(window:string):string {
  const key=window.toLowerCase().replace(/[\s_-]/g,'');
  if(['weekly','week','1w','7d','7days'].includes(key))return 'Weekly limit';
  if(['5h','5hr','5hrs','5hour','5hours','fivehour','fivehours'].includes(key))return '5h limit';
  return window;
}

export function quotaGroups(agent:Agent) {
  type Row={quota:Quota; window:string; bucket:string};
  const groups=new Map<string,Row[]>();
  for(const quota of agent.quotas){
    // Only this adapter encodes group, bucket and window in a validated label.
    const parts=agent.id==='antigravity'?quota.label.split(' · '):[];
    const [group,bucket,window]=parts.length===3?parts:['','',quota.label];
    if(!groups.has(group))groups.set(group,[]);
    groups.get(group)!.push({quota,window:group?compactWindow(window):window,bucket});
  }
  return [...groups].map(([group,rows])=>({
    group,rows:rows.map(({quota,window,bucket})=>{
      const peers=rows.filter(row=>row.window===window);
      let label=window;
      if(peers.length>1){
        if(bucket)label+=` · ${bucket}`;
        if(peers.filter(row=>row.bucket===bucket).length>1)label+=` · ${quota.id}`;
      }
      return {quota,label};
    })
  }));
}

/** UI projection only: native identity, authentication and source data stay unchanged. */
export function presentAgent(agent:Agent):Agent {
  if(agent.id!=='antigravity')return agent;
  return {...agent,name:'Gemini'};
}

export function usageTools(agents: Agent[]): Agent[] {
  return agents.filter(a => a.installed).map(presentAgent);
}
