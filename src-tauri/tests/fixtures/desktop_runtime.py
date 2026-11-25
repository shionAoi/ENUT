import os,pathlib,hashlib,json
r=pathlib.Path(os.environ['VERIFY_FIXTURES']);r.mkdir();(r/'tmp').mkdir()
script='''#!/usr/bin/python3 -I
import os,sys,json
from pathlib import Path
r=Path(os.environ['VERIFY_ROOT']); mode=os.environ['VERIFY_MODE']
bin=str(r/'home/.local/share/fnm/aliases/default/bin')
expected=str(r/'explicit/codex') if mode=='priority' else bin+'/codex'
assert sys.argv[1:]==[expected,'app-server'],sys.argv
assert os.environ['HOME']==str(r/'home')
assert os.environ['CODEX_HOME']==str(r/'auth')
paths=os.environ['PATH'].split(':')
assert paths==([str(r/'explicit'),bin] if mode=='terminal' else [str(r/'explicit')] if mode=='priority' else [bin,str(r/'explicit')]),paths
for line in sys.stdin:
 q=json.loads(line);m=q['method']
 with open(r/'rpc.log','a') as f:f.write(m+'\\n')
 if m=='initialize':v={}
 elif m=='initialized':continue
 elif m=='account/read':
  assert q['params']=={'refreshToken':False};v={'account':{'type':'chatgpt'}}
 elif m=='account/rateLimits/read':v={'rateLimits':{'primary':{'usedPercent':42}}}
 elif m=='account/login/start':
  assert q['params']=={'type':'chatgptDeviceCode'};v={'type':'chatgptDeviceCode','loginId':'v','verificationUrl':'https://auth.openai.com/codex/device','userCode':'FAKE'}
 elif m=='account/login/cancel':
  assert q['params']=={'loginId':'v'};v={'status':'canceled'}
 else:raise AssertionError(m)
 print(json.dumps({'id':q['id'],'result':v}),flush=True)
'''
def put(p,s,mode=0o600):p.write_text(s);p.chmod(mode)
for mode in ['desktop','terminal','priority','noexec','broken','missingnode']:
 b=r/mode;h=b/'home';fnm=h/'.local/share/fnm';install=fnm/'node-versions/v-test/installation';bin=install/'bin';bin.mkdir(parents=True)
 (fnm/'aliases').mkdir();(fnm/'aliases/default').symlink_to('../node-versions/v-test/installation')
 package=install/'lib/node_modules/@openai/codex/bin';package.mkdir(parents=True)
 put(package/'codex.js','#!/usr/bin/env node\n',0o600 if mode=='noexec' else 0o700)
 (bin/'codex').symlink_to('../lib/node_modules/@openai/codex/bin/'+('absent.js' if mode=='broken' else 'codex.js'))
 if mode!='missingnode':put(bin/'node',script,0o700)
 (b/'explicit').mkdir();(b/'empty').mkdir();(b/'auth').mkdir()
 put(b/'auth/auth.json','synthetic-secret-do-not-modify\n')
 put(h/'.profile','echo forbidden > '+str(b/'profile-executed')+'\n')
 put(h/'.bashrc','echo forbidden > '+str(b/'profile-executed')+'\n')
 if mode in ['terminal','priority']:put(b/'explicit/node',script,0o700)
 if mode=='priority':put(b/'explicit/codex','#!/usr/bin/env node\n',0o700)
def hashes():return {str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in r.rglob('*') if p.is_file() and not p.is_symlink() and p.name not in ['rpc.log','before.json']}
(r/'before.json').write_text(json.dumps(hashes()))
