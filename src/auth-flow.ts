export interface AuthChoice { id:string;provider:string;method:string;label:string;kind:string }
export interface AuthView {attemptId:string;toolId:string;state:string;kind:string;message:string;output:string;url:string|null;userCode:string|null}
export type AuthInvoke=(command:string,args?:Record<string,unknown>)=>Promise<unknown>;
export const terminalAuth=(state:string)=>['authenticated','finished','failed','expired','cancelled'].includes(state);
export class AuthFlow {
 view:AuthView|null=null;
 choices:AuthChoice[]=[];
 tool='';
 private epoch=0;
 private pendingStart:Promise<AuthView>|null=null;
 private pendingCancelId:string|null=null;
 private invoke:AuthInvoke;
 constructor(invoke:AuthInvoke){this.invoke=invoke;}
 async load(tool:string){
  await this.cancel();const epoch=++this.epoch;this.tool=tool;
  const choices=await this.invoke('auth_methods',{agentId:tool}) as AuthChoice[];
  if(epoch===this.epoch)this.choices=choices;
 }
 async start(choiceId:string){
  if(['ollama','lmstudio'].includes(this.tool))throw Error('Local runtime login is not required.');
  if(this.pendingStart)throw Error('A sign-in is already starting.');
  const epoch=++this.epoch;
  const request=this.invoke('auth_start',{agentId:this.tool,choiceId}) as Promise<AuthView>;
  this.pendingStart=request;
  try{const view=await request;if(epoch===this.epoch)this.view=view;}
  finally{if(this.pendingStart===request)this.pendingStart=null;}
 }
 async poll(){
  if(!this.view||terminalAuth(this.view.state))return;
  const id=this.view.attemptId,epoch=this.epoch;
  const view=await this.invoke('auth_poll',{attemptId:id}) as AuthView;
  if(epoch===this.epoch&&this.view?.attemptId===id)this.view=view;
 }
 async submit(code:string){
  if(!this.view||this.view.kind!=='native'||this.view.state!=='waiting')throw Error('This sign-in does not accept a pasted code.');
  if(!/^[A-Za-z0-9._~:/?&=%+#@-]{1,1024}$/.test(code))throw Error('Paste one authorization code or redirect URL only.');
  try{await this.invoke('auth_submit',{attemptId:this.view.attemptId,code});}finally{code='';}
 }
 async openBrowser(){if(!this.view?.url)throw Error('No browser challenge is active.');await this.invoke('auth_open_browser',{attemptId:this.view.attemptId});}
 async cancel(){
  ++this.epoch;let view=this.view;const pending=this.pendingStart;this.view=null;this.choices=[];
  if(pending){try{view=await pending;}catch{/* A rejected start created no owned attempt. */}}
  const id=view?.attemptId??this.pendingCancelId;
  if(id){this.pendingCancelId=id;await this.invoke('auth_cancel',{attemptId:id});this.pendingCancelId=null;}
 }
}
