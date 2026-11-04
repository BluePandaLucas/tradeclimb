#!/usr/bin/env python3
"""Exercise owner login, agent scope/revocation, and optional real Unix-account isolation."""
import argparse,http.client,http.cookiejar,json,os,pathlib,signal,socket,subprocess,tempfile,time,urllib.error,urllib.request,uuid
ROOT=pathlib.Path(__file__).resolve().parent.parent
parser=argparse.ArgumentParser();parser.add_argument('--agent-account');args=parser.parse_args()
with tempfile.TemporaryDirectory(prefix='tc-security-') as folder:
 root=pathlib.Path(folder);root.chmod(0o755);data=root/'data';binary=ROOT/'target/release/tc'
 def cli(*arguments):
  result=subprocess.run([str(binary),'--data-dir',str(data),*arguments],cwd=ROOT,text=True,capture_output=True,check=True)
  return json.loads(result.stdout)
 demo=cli('demo');owner=(data/'auth/owner.key').read_text().strip()
 credential_file=root/'agent.key';issued=cli('auth','issue-agent','--name','integration-agent','--output',str(credential_file));agent=credential_file.read_text().strip()
 private=data/'private-probe';private.write_text('owner-only-marker');private.chmod(0o644)
 with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
 base=f'http://127.0.0.1:{port}'
 def request(path,key=None,body=None,opener=None):
  headers={'Content-Type':'application/json'}
  if key:headers['Authorization']='Bearer '+key
  req=urllib.request.Request(base+path,headers=headers,data=None if body is None else json.dumps(body).encode())
  try:
   with (opener.open(req,timeout=10) if opener else urllib.request.urlopen(req,timeout=10)) as response:return response.status,json.load(response),response.headers
  except urllib.error.HTTPError as error:return error.code,json.load(error),error.headers
 def until(fn):
  end=time.monotonic()+60
  while time.monotonic()<end:
   try:
    result=fn()
    if result:return result
   except OSError:pass
   time.sleep(.1)
  raise AssertionError('security workflow timed out')
 with open(root/'service.log','w+') as log:
  service=subprocess.Popen([str(binary),'--data-dir',str(data),'serve','--bind',f'127.0.0.1:{port}','--assets',str(ROOT/'dist')],cwd=ROOT,stdout=log,stderr=log)
  try:
   until(lambda:request('/api/overview',owner)[0]==200)
   assert request('/api/overview')[0]==401
   assert request('/api/events')[0]==401
   assert request('/login',body={'key':agent})[0]==401
   cookies=http.cookiejar.CookieJar();browser=urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cookies))
   status,_,headers=request('/login',body={'key':owner},opener=browser);assert status==200
   cookie=headers.get('Set-Cookie','');assert 'HttpOnly' in cookie and 'SameSite=Strict' in cookie and 'Max-Age=43200' in cookie
   assert request('/api/session',opener=browser)[1]['role']=='owner'
   overview=request('/api/overview',agent)[1];assert 'validation' not in overview and 'paper' not in overview and 'paper_approvals' not in overview
   for path in ['/api/paper','/api/campaigns','/api/infer']:
    assert request(path,agent,{})[0]==403
   assert request('/api/jobs',agent,{'strategy':demo['strategies'][0]['strategy'],'campaign':demo['campaign'],'period':'validation','idempotency_key':'denied'})[0]==403
   assert request('/api/overview',owner)[1]['validation']['used']==0
   dev={'strategy':demo['strategies'][0]['strategy'],'campaign':demo['campaign'],'period':'development','idempotency_key':'agent-work'}
   status,job,_=request('/api/jobs',agent,dev);assert status==200
   repeat=request('/api/jobs',agent,dev)[1];assert repeat['id']==job['id']
   validation=request('/api/jobs',owner,{**dev,'period':'validation','idempotency_key':'owner-validation'})[1]
   assert request('/api/jobs/'+validation['id'],agent)[0]==403
   if args.agent_account:
    code="""import json,os,pathlib,urllib.request,urllib.error,sys
p=json.load(sys.stdin)
assert os.getuid()!=0
try:pathlib.Path(p['private']).read_text();denied=False
except PermissionError:denied=True
r=urllib.request.Request(p['url']+'/api/overview',headers={'Authorization':'Bearer '+p['key']})
with urllib.request.urlopen(r,timeout=10) as response:body=json.load(response)
print(json.dumps({'private_read_denied':denied,'validation_visible':'validation' in body,'paper_visible':'paper' in body}))
"""
    result=subprocess.run(['sudo','-n','-u',args.agent_account,'--','/usr/bin/python3','-c',code],cwd='/tmp',input=json.dumps({'private':str(private),'url':base,'key':agent}),text=True,capture_output=True,check=True)
    probe=json.loads(result.stdout);assert probe=={'private_read_denied':True,'validation_visible':False,'paper_visible':False}
   endpoint=json.loads((data/'auth/agent-endpoint.json').read_text())['socket']
   def unix_request(path,key):
    connection=http.client.HTTPConnection('localhost',timeout=5)
    connection.sock=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);connection.sock.connect(endpoint)
    connection.request('GET',path,headers={'Authorization':'Bearer '+key})
    response=connection.getresponse();status=response.status;response.read();connection.close();return status
   assert unix_request('/api/overview',owner)==403
   assert unix_request('/login',agent)==403
   if args.agent_account:
    token_name='security-'+uuid.uuid4().hex+'.key'
    agent_token='/var/lib/'+args.agent_account+'/'+token_name
    subprocess.run(['sudo','-n','install','-o',args.agent_account,'-g',args.agent_account,'-m','600',str(credential_file),agent_token],check=True)
    code="""import http.client,json,os,pathlib,socket
assert 'TC_OWNER_SENTINEL' not in os.environ
assert not pathlib.Path('/home').exists()
tcp=socket.socket();tcp.settimeout(.2);assert tcp.connect_ex(('127.0.0.1',PORT))!=0
connection=http.client.HTTPConnection('localhost',timeout=5)
connection.sock=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);connection.sock.connect(os.environ['TC_API_SOCKET'])
key=pathlib.Path(os.environ['TC_API_TOKEN_FILE']).read_text().strip()
connection.request('GET','/api/overview',headers={'Authorization':'Bearer '+key})
response=connection.getresponse();assert response.status==200
body=json.load(response);assert 'validation' not in body and 'paper' not in body
print('agent-network-and-filesystem-isolated')
""".replace('PORT',str(port))
    try:
     result=subprocess.run([str(ROOT/'scripts/run-agent'),'/usr/bin/python3','-c',code],cwd=ROOT,env=dict(os.environ,TC_DATA_DIR=str(data),TC_AGENT_ACCOUNT=args.agent_account,TC_AGENT_TOKEN_NAME=token_name,TC_OWNER_SENTINEL='must-not-inherit'),capture_output=True,text=True,check=True)
     assert 'agent-network-and-filesystem-isolated' in result.stdout
    finally:subprocess.run(['sudo','-n','rm','-f',agent_token],check=True)
   cli('auth','revoke',issued['id']);assert request('/api/overview',agent)[0]==401
   assert request('/api/logout',body={},opener=browser)[0]==200
   assert request('/api/session',opener=browser)[0]==401
   request('/login',body={'key':owner},opener=browser)
   cli('auth','rotate-owner');assert request('/api/session',opener=browser)[0]==401
   assert request('/api/overview',owner)[0]==401
  finally:
   service.send_signal(signal.SIGINT);service.wait(timeout=8);assert service.returncode==0
print('PASS: private owner sessions, development-only agents, budget protection, revocation, logout/rotation, and optional Unix-account separation')
