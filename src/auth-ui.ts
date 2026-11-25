import {invoke} from '@tauri-apps/api/core';
import {AuthFlow,terminalAuth} from './auth-flow.ts';
import {tr,type Lang,type TranslationKey} from './i18n.ts';

export function mountAuth(root:HTMLElement,desktop:boolean,pause:(tool:string)=>Promise<void>,refresh:()=>Promise<void>,getLang:()=>Lang=()=>'en'){
 const flow=new AuthFlow((command,args)=>invoke(command,args));
 const dialog=document.createElement('dialog');dialog.id='native-auth';dialog.setAttribute('aria-labelledby','auth-title');
  dialog.innerHTML=`<h2 id="auth-title">Sign in</h2><p id="auth-intro-1" class="meta">Native sign-in can open your browser and saves credentials in the selected tool. It may change that tool’s active account. Cancelling here does not log out an existing account.</p><p id="auth-intro-2" class="meta">Monitoring pauses during login. After finishing, use Connect to approve quota access for the signed-in account.</p><p id="auth-error" role="status"></p><div id="auth-choices"><label id="auth-provider-label" for="auth-provider">Provider / account service</label><select id="auth-provider"></select><p id="auth-provider-hint" class="meta">Choose the account on the provider’s sign-in page; no password is entered here.</p><label id="auth-method-label" for="auth-method">Sign-in method</label><select id="auth-method"></select><button id="auth-start" type="button" class="primary">Start native sign-in</button></div><section id="auth-progress" hidden><p id="auth-message" role="status"></p><p id="auth-url" class="meta"></p><p id="auth-device"></p><button id="auth-open" type="button" hidden>Open provider sign-in page</button><pre id="auth-output" aria-label="Native sign-in instructions"></pre><section id="auth-paste"><h3 id="auth-paste-title">Return here after browser sign-in</h3><p class="meta">If Antigravity or another provider asks you to paste a login code back, enter that authorization code or redirect URL here and select Submit code to native tool. The native tool saves the login; this monitor does not store the code. Device codes must be entered on the provider website instead. Never paste a password, API key or access token.</p><form id="auth-code-form"><label id="auth-code-label" for="auth-code">Authorization code or redirect URL</label><input id="auth-code" type="password" autocomplete="off" autocapitalize="none" autocorrect="off" spellcheck="false" maxlength="1024"><button id="auth-submit" type="submit">Submit code to native tool</button></form></section></section><div class="dialog-actions"><button id="auth-close" type="button">Cancel / close</button></div>`;
  root.append(dialog);
 const get=<T extends HTMLElement=HTMLElement>(id:string)=>dialog.querySelector<T>(`#${id}`)!;
 const provider=get<HTMLSelectElement>('auth-provider'),method=get<HTMLSelectElement>('auth-method');
 get('auth-close').setAttribute('autofocus','');
 get('auth-close').setAttribute('aria-describedby','auth-error');
 const codeHint=get('auth-paste').querySelector('p')!;codeHint.id='auth-code-hint';
 get('auth-code').setAttribute('aria-describedby','auth-code-hint auth-error');
 let timer:ReturnType<typeof setTimeout>|undefined,busy=false,serial=0;
 let finishConnection:(()=>Promise<void>)|undefined;
 let activeTool:string|null=null;
 let errorKey:TranslationKey|null=null;
 const error=(text:string)=>{errorKey=null;get('auth-error').textContent=text;};
 const setErrorKey=(key:TranslationKey|null)=>{errorKey=key;get('auth-error').textContent=key?tr(getLang(),key):'';};
 function syncLang(){
  const lang=getLang();
  get('auth-title').textContent=finishConnection&&activeTool
   ?tr(lang,'authLoginTo',{tool:activeTool==='codex'?'Codex':activeTool==='antigravity'?'Gemini':activeTool})
   :tr(lang,'authTitle');
  get('auth-intro-1').textContent=tr(lang,'authIntro1');
  get('auth-intro-2').textContent=tr(lang,'authIntro2');
  get('auth-provider-label').textContent=tr(lang,'authProviderLabel');
  get('auth-provider-hint').textContent=tr(lang,'authProviderHint');
  get('auth-method-label').textContent=tr(lang,'authMethodLabel');
  get('auth-start').textContent=tr(lang,'authStartBtn');
  get('auth-open').textContent=tr(lang,'authOpenBtn');
  get('auth-output').setAttribute('aria-label',tr(lang,'authOutputAria'));
  get('auth-paste-title').textContent=tr(lang,'authPasteHeading');
  codeHint.textContent=tr(lang,'authPasteBody');
  get('auth-code-label').textContent=tr(lang,'authCodeLabel');
  get('auth-submit').textContent=tr(lang,'authSubmitBtn');
  if(errorKey)get('auth-error').textContent=tr(lang,errorKey);
  if(flow.view&&terminalAuth(flow.view.state)){
   get('auth-close').textContent=tr(lang,finishConnection&&['finished','authenticated'].includes(flow.view.state)?'authFinishConnect':'authFinishClose');
  }else{
   get('auth-close').textContent=tr(lang,'authCancelClose');
  }
  if(flow.view)render();
 }
 function methods(){method.replaceChildren();for(const c of flow.choices.filter(c=>c.provider===provider.value))method.add(new Option(c.label,c.id));get<HTMLButtonElement>('auth-start').disabled=!method.options.length;}
 function render(){
  const lang=getLang();
  const v=flow.view;get('auth-choices').hidden=!!v;get('auth-progress').hidden=!v;
  if(!v)return;
  get('auth-message').textContent=v.message;
  get('auth-url').textContent=v.url??'';
  get('auth-device').textContent=v.userCode?tr(lang,'authDeviceCodePrompt',{code:v.userCode}):'';
  get('auth-output').textContent=v.output;
  get('auth-open').hidden=!v.url;
  get('auth-paste').hidden=v.kind!=='native'||v.state!=='waiting';
   if(terminalAuth(v.state)){
    get<HTMLInputElement>('auth-code').value='';
    get('auth-close').textContent=tr(lang,finishConnection&&['finished','authenticated'].includes(v.state)?'authFinishConnect':'authFinishClose');
    if(finishConnection&&v.state==='finished')get('auth-message').textContent=tr(lang,'authFinishedNotice');
   }
 }
 async function poll(){
  const own=serial;if(!dialog.open||!flow.view||terminalAuth(flow.view.state))return;
  try{await flow.poll();if(own!==serial)return;render();if(flow.view&&!terminalAuth(flow.view.state))timer=setTimeout(()=>void poll(),700);}
  catch{setErrorKey('authPollError');}
 }
 async function close(activate=false){
  const complete=activate&&flow.view&&['finished','authenticated'].includes(flow.view.state)?finishConnection:undefined;
  finishConnection=undefined;activeTool=null;
   ++serial;clearTimeout(timer);get<HTMLInputElement>('auth-code').value='';get('auth-output').textContent='';get('auth-device').textContent='';get('auth-url').textContent='';
  try{await flow.cancel();}catch{setErrorKey('authCancelError');return;}
  if(dialog.open)dialog.close();await refresh();
  if(complete)await complete();
 }
 async function open(tool:string,onFinished?:()=>Promise<void>){
  if(!desktop||busy||dialog.open)return;
  finishConnection=onFinished;activeTool=tool;
  syncLang();
  ++serial;clearTimeout(timer);busy=true;setErrorKey('authLoadingMethods');get('auth-close').textContent=tr(getLang(),'authCancelClose');get('auth-choices').hidden=false;get('auth-progress').hidden=true;provider.replaceChildren();method.replaceChildren();get<HTMLButtonElement>('auth-start').disabled=true;dialog.showModal();
  const own=serial;
  try{
   await flow.load(tool);if(own!==serial)return;
   if(onFinished){
    flow.choices=flow.choices.filter(c=>tool==='codex'
     ?c.provider==='chatgpt'&&c.method==='device'&&c.kind==='device_code'
     :tool==='antigravity'?c.provider==='google'&&c.method==='native'&&c.kind==='native'
     :c.kind==='native');
   }
   for(const p of new Set(flow.choices.map(c=>c.provider)))provider.add(new Option(p,p));methods();
   if(flow.choices.length)error('');else setErrorKey('authNoMethods');
   if(onFinished){
    if(!flow.choices.length)throw Error('Unsupported native login');
    if(['codex','antigravity'].includes(tool)){
     if(flow.choices.length!==1)throw Error('Ambiguous native login');
     await start(flow.choices[0].id,own);
    }
   }
  }catch{if(own===serial){flow.choices=[];provider.replaceChildren();method.replaceChildren();get<HTMLButtonElement>('auth-start').disabled=true;setErrorKey('authStartError');}}
  finally{busy=false;}
 }
 async function start(choice:string,own:number){
  await pause(flow.tool);if(own!==serial)throw Error('Cancelled');
  await flow.start(choice);
  if(own===serial){render();void poll();}
 }
 root.addEventListener('click',event=>{
  const button=(event.target as Element).closest<HTMLButtonElement>('[data-auth-tool]');if(!button)return;
  void open(button.dataset.authTool!);
 });
 provider.addEventListener('change',methods);
 get('auth-start').addEventListener('click',()=>{
  if(busy||!method.value)return;busy=true;get<HTMLButtonElement>('auth-start').disabled=true;error('');const own=serial;
  void start(method.value,own).catch(()=>{if(own===serial)setErrorKey('authStartPausedError');}).finally(()=>{busy=false;});
 });
 get('auth-open').addEventListener('click',()=>{void flow.openBrowser().catch(()=>setErrorKey('authOpenBrowserError'));});
 get<HTMLFormElement>('auth-code-form').addEventListener('submit',event=>{
  event.preventDefault();const field=get<HTMLInputElement>('auth-code');let code=field.value;field.value='';
   void flow.submit(code).then(()=>{setErrorKey('authCodeSent');}).catch(()=>setErrorKey('authCodeSubmitError'));code='';
 });
 get('auth-close').addEventListener('click',()=>{void close(true);});
 dialog.addEventListener('cancel',event=>{event.preventDefault();void close();});
 syncLang();
 return {openConnect:(tool:string,onFinished:()=>Promise<void>)=>open(tool,onFinished),setLang:()=>syncLang()};
}
