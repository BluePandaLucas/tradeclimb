#!/usr/bin/env python3
"""Verify benchmark/chart presentation and owner-only validation with synthetic fixtures."""
import json,pathlib,signal,socket,subprocess,tempfile,time,urllib.error,urllib.request
from playwright.sync_api import sync_playwright,expect
ROOT=pathlib.Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='tc-research-ui-') as folder:
 state=pathlib.Path(folder);binary=ROOT/'target/release/tc'
 def cli(*args):return json.loads(subprocess.run([str(binary),'--data-dir',folder,*args],capture_output=True,text=True,check=True).stdout)
 demo=cli('demo');owner=(state/'auth/owner.key').read_text().strip()
 token=state/'agent.key';cli('auth','issue-agent','--name','research-test','--output',str(token));agent=token.read_text().strip()
 with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
 base=f'http://127.0.0.1:{port}'
 def request(path,key=owner,body=None):
  req=urllib.request.Request(base+path,headers={'Authorization':'Bearer '+key,'Content-Type':'application/json'},data=None if body is None else json.dumps(body).encode())
  try:
   with urllib.request.urlopen(req,timeout=30) as response:return response.status,json.load(response)
  except urllib.error.HTTPError as error:return error.code,json.load(error)
 def until(fn):
  deadline=time.monotonic()+60
  while time.monotonic()<deadline:
   try:
    result=fn()
    if result:return result
   except OSError:pass
   time.sleep(.1)
  raise AssertionError('research fixture timed out')
 with tempfile.TemporaryFile(mode='w+') as log:
  server=subprocess.Popen([str(binary),'--data-dir',folder,'serve','--bind',f'127.0.0.1:{port}','--assets',str(ROOT/'dist')],cwd=ROOT,stdout=log,stderr=log)
  try:
   data=until(lambda: (lambda d:d if d and all(j['state']=='completed' for j in d['jobs']) else None)(request('/api/overview')[1]))
   version=next(s['id'] for s in data['strategies'] if s['spec']['name']=='Trend explorer')
   job=next(j for j in data['jobs'] if j['strategy']==version)
   original=request('/api/jobs/'+job['id'])[1]['result'];assert len(original['benchmarks'])==2
   assert {b['id'] for b in original['benchmarks']}=={'btc_buy_hold_v1','five_asset_buy_hold_v1'}
   assert all(b['comparison']['paired_days']>2 for b in original['benchmarks'])
   # Presentation-only density metadata; raw strategy accounting/trades stay intact.
   shown=dict(original);shown['marker_total']=501
   (state/'jobs'/job['id']/'result.json').write_text(json.dumps(shown))
   with sync_playwright() as p:
    browser=p.chromium.launch(headless=True,args=['--no-sandbox']);page=browser.new_page(viewport={'width':1440,'height':1000})
    errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
    page.goto(base+'/login');page.get_by_label('Owner key').fill(owner);page.get_by_role('button',name='Sign in',exact=True).click()
    page.get_by_role('heading',name='Overview',exact=True).wait_for();page.get_by_role('button',name='Backtests',exact=True).click()
    search=page.get_by_role('searchbox',name='Search runs');search.fill(job['id'])
    row=page.locator(f'tr[data-run-id="{job["id"]}"]');row.click()
    chart=page.locator('.run-detail');chart.get_by_role('img',name='Net portfolio equity').wait_for()
    expect(chart.locator('.benchmark-line')).to_have_count(2)
    for line in chart.locator('.benchmark-line').all():
     assert line.get_attribute('stroke-dasharray') is None
     assert line.get_attribute('stroke-opacity')=='0.5'
    expect(chart.locator('.trade-marker')).to_have_count(0);assert '501' in chart.locator('.marker-note').inner_text()
    expect(chart.locator('.benchmark-comparison tbody tr')).to_have_count(2)
    page.screenshot(path='/tmp/tradeclimb-v2-research.png',full_page=True)
    assert request('/api/jobs/'+job['id']+'/trades')[1]['total']==original['metrics']['trades']
    chart.get_by_label('BTC reference',exact=True).uncheck();expect(chart.locator('.benchmark-line')).to_have_count(1)
    assert chart.locator('.benchmark-line').get_attribute('stroke')=='#c1b49c'
    expect(chart.locator('.benchmark-comparison tbody tr')).to_have_count(2)
    status,validation=request('/api/jobs',body={'strategy':version,'campaign':job['campaign'],'period':'validation','idempotency_key':'owner-inspection'})
    assert status==200;vid=validation['id']
    until(lambda:request('/api/jobs/'+vid)[1]['state']=='completed')
    assert request('/api/jobs/'+vid)[1]['owner_detail'] is False
    before=request('/api/overview')[1]['validation'];assert before['revealed'] is False
    for path,body in [('/api/jobs/'+vid,None),('/api/jobs/'+vid+'/trades',None),('/api/jobs/'+vid+'/reveal',{})]:assert request(path,agent,body)[0]==403
    page.wait_for_timeout(1200);search.fill(vid);page.locator(f'tr[data-run-id="{vid}"]').click()
    button=page.get_by_role('button',name='View validation details',exact=True);button.wait_for()
    assert chart.get_by_role('img',name='Net portfolio equity').count()==0
    button.click();chart.get_by_role('img',name='Net portfolio equity').wait_for()
    expect(chart.locator('.benchmark-comparison tbody tr')).to_have_count(2)
    details=request('/api/jobs/'+vid)[1];assert details['owner_detail'] and details['result']['equity']
    assert request('/api/jobs/'+vid+'/trades')[0]==200
    after=request('/api/overview')[1]['validation'];assert after['used']==before['used'] and after['revealed']
    for path in ['/api/jobs/'+vid,'/api/jobs/'+vid+'/trades']:assert request(path,agent)[0]==403
    assert vid not in json.dumps(request('/api/jobs/index',agent)[1])
    view=request('/api/overview',agent)[1];assert vid not in json.dumps(view) and 'validation' not in view and view['job_counts']['total']==3
    page.get_by_label('Chart window').select_option('7');page.wait_for_timeout(1200);assert chart.get_by_role('img',name='Net portfolio equity').count()==1
    assert cli('final-test','status')['state']=='sealed'
    # Mock native provenance solely to test picker eligibility and archived selections.
    manifest_path=next((state/'datasets').glob('*/manifest.json'));manifest=json.loads(manifest_path.read_text());manifest.update(synthetic=False,venue='bitvavo');manifest_path.write_text(json.dumps(manifest))
    ready=until(lambda:(lambda d:d if d.get('paper_candidates') else None)(request('/api/overview')[1]))
    archived=ready['paper_candidates'][0]['strategy'];assert request('/api/strategies/'+archived+'/archive',body={})[0]==200
    page.get_by_role('button',name='Paper trading',exact=True).click()
    control=page.locator('.paper-grid>button');control.wait_for();page.wait_for_timeout(1200)
    assert control.get_attribute('data-strategy-id')!=archived
    for s in ready['strategies']:
     if s['id']!=archived:assert request('/api/strategies/'+s['id']+'/archive',body={})[0]==200
    expect(control).to_be_disabled();page.wait_for_timeout(1200)
    page.get_by_role('button',name='Backtests',exact=True).click();search.wait_for();assert not errors,errors
    browser.close()
  finally:server.send_signal(signal.SIGINT);server.wait(timeout=8);assert server.returncode==0
print('PASS: solid translucent references, dense-marker suppression without lost trades, benchmark diagnostics, audited owner validation access and persistent agent restrictions')
