#!/usr/bin/env python3
"""Optional headless browser checks. Requires Playwright and its Chromium runtime."""
import json,pathlib,signal,socket,subprocess,tempfile,time,urllib.request
from playwright.sync_api import sync_playwright
root=pathlib.Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='tc-ui-') as folder:
 state=pathlib.Path(folder)
 subprocess.run([str(root/'target/release/tc'),'--data-dir',folder,'demo'],check=True,capture_output=True)
 with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
 owner_key=(state/'auth/owner.key').read_text().strip()
 def api_request(url,**kwargs):
  return urllib.request.Request(url,headers={'Authorization':'Bearer '+owner_key,'Content-Type':'application/json'},**kwargs)
 service=subprocess.Popen([str(root/'target/release/tc'),'--data-dir',folder,'serve','--bind',f'127.0.0.1:{port}','--assets',str(root/'dist')],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 try:
  for _ in range(100):
   try:
    with urllib.request.urlopen(api_request(f'http://127.0.0.1:{port}/api/overview')) as response:data=json.load(response)
    if all(j['state']=='completed' for j in data['jobs']):break
   except OSError:pass
   time.sleep(.2)
  with sync_playwright() as p:
   browser=p.chromium.launch(headless=True,args=['--no-sandbox'])
   page=browser.new_page(viewport={'width':1440,'height':1000},color_scheme="dark")
   errors=[]
   page.on('pageerror',lambda e:errors.append(str(e)))
   page.goto(f'http://127.0.0.1:{port}/login',wait_until='domcontentloaded')
   page.get_by_label('Owner key').fill(owner_key)
   page.get_by_role('button',name='Sign in',exact=True).click()
   page.get_by_role('heading',name='Overview',exact=True).wait_for()
   assert page.locator('.stats').count()==1
   assert page.locator('.controls').count()==0
   page.get_by_role('button',name='Collapse sidebar',exact=True).click()
   page.wait_for_function('document.querySelector(".sidebar").getBoundingClientRect().width === 64')
   assert page.get_by_role('button',name='Expand sidebar',exact=True).get_attribute('aria-expanded')=='false'
   page.reload(wait_until='domcontentloaded')
   page.get_by_role('button',name='Expand sidebar',exact=True).wait_for()
   assert page.locator('.shell').evaluate('el => el.classList.contains("sidebar-collapsed")')
   page.get_by_role('button',name='Expand sidebar',exact=True).focus()
   page.keyboard.press('Enter')
   page.wait_for_function('document.querySelector(".sidebar").getBoundingClientRect().width === 216')
   toggle=page.get_by_role('button',name='Toggle dark mode',exact=True)
   system=page.get_by_role('button',name='Use system theme',exact=True)
   toggle.wait_for()
   assert page.locator('html').get_attribute('data-theme')=='system'
   assert toggle.get_attribute('aria-pressed')=='true'
   assert toggle.locator('svg circle').count()==1
   assert toggle.inner_text()==''
   assert page.evaluate('getComputedStyle(document.body).backgroundColor') in ('rgba(0, 0, 0, 0)','rgb(17, 26, 30)')
   assert page.evaluate('getComputedStyle(document.documentElement).backgroundColor')=='rgb(17, 26, 30)'
   assert '0.22s' in page.locator('html').evaluate('el => getComputedStyle(el).transitionDuration')
   page.emulate_media(reduced_motion='reduce')
   assert page.locator('html').evaluate('el => getComputedStyle(el).transitionDuration')=='0s'
   page.emulate_media(reduced_motion='no-preference')
   toggle.click()
   assert page.locator('html').get_attribute('data-theme')=='light'
   assert toggle.locator('svg').count()==1
   assert toggle.locator('svg circle').count()==0
   assert toggle.inner_text()==''
   page.reload(wait_until='domcontentloaded')
   toggle.wait_for()
   assert page.locator('html').get_attribute('data-theme')=='light'
   system.click()
   page.emulate_media(color_scheme='light')
   page.wait_for_function('getComputedStyle(document.documentElement).backgroundColor === "rgb(243, 246, 246)"')
   page.emulate_media(color_scheme='dark')
   page.wait_for_function('getComputedStyle(document.documentElement).backgroundColor === "rgb(17, 26, 30)"')
   page.get_by_role('button',name='Trend explorer',exact=True).first.wait_for()
   page.get_by_role('button',name='Backtests',exact=True).click()
   assert page.locator('.stats').count()==0
   assert page.locator('.controls').count()==1
   assert 'Synthetic' in page.get_by_label('Evaluation campaign').inner_text()
   picker=page.locator('.controls .picker-trigger')
   strategy_control=page.locator('.controls .strategy-control')
   initial=next(strategy['id'] for strategy in data['strategies'] if strategy['id'][:8] in strategy_control.inner_text())
   initial_label=strategy_control.inner_text()
   subprocess.run([str(root/'target/release/tc'),'--data-dir',folder,'builtin','--name','New fixture version'],check=True,capture_output=True)
   page.wait_for_timeout(2200)
   assert strategy_control.inner_text()==initial_label,'default selection changed while new versions arrived'
   page.get_by_role('button',name='＋ Run backtest',exact=True).click()
   page.wait_for_timeout(1500)
   with urllib.request.urlopen(api_request(f'http://127.0.0.1:{port}/api/overview')) as response:submitted=json.load(response)
   assert submitted['jobs'][0]['strategy']==initial,'submitted strategy differs from visible selection'
   value=next(strategy['id'] for strategy in data['strategies'] if strategy['id']!=initial)
   picker.click()
   page.get_by_role('searchbox',name='Search strategies and versions').fill(value)
   page.locator('dialog[open] .version-option').click()
   page.wait_for_timeout(2200)
   assert value[:8] in page.locator('.controls .strategy-control').inner_text(),'strategy selection reset after stream update'
   arrivals=page.get_by_role('button',name='Show',exact=False).filter(has_text='new runs')
   if arrivals.count():arrivals.click()
   page.get_by_role('button',name='Trend explorer',exact=True).first.click()
   page.get_by_role('img',name='Net portfolio equity').wait_for()
   page.get_by_label('Chart window').select_option('7')
   page.wait_for_timeout(2200)
   assert page.get_by_label('Chart window').input_value()=='7'
   page.get_by_label('BTC buy & hold',exact=True).uncheck()
   page.wait_for_timeout(1200)
   assert not page.get_by_label('BTC buy & hold',exact=True).is_checked()
   page.evaluate('window.scrollTo(0,0)')
   page.screenshot(path='/tmp/tradeclimb-desktop.png',full_page=True)
   # A tuning experiment stays one summary; its many trials are opt-in.
   evaluation_rows=page.locator('.evaluations tbody tr')
   baseline=evaluation_rows.count()
   original=page.locator('.run-detail .panel-heading code').inner_text()
   page.evaluate('window._chartNode=document.querySelector(".run-detail svg")')
   tune_requests=[]
   page.on('request',lambda request:tune_requests.append(request.url) if request.method=='POST' and request.url.endswith('/api/tune') else None)
   tune=page.get_by_role('button',name='Tune parameters · 20 trials',exact=True)
   tune.evaluate('el => { el.click(); el.click(); }')
   deadline=time.monotonic()+100
   while time.monotonic()<deadline:
    with urllib.request.urlopen(api_request(f'http://127.0.0.1:{port}/api/overview')) as response:tuning_data=json.load(response)
    if any(len(t.get('jobs',[]))>=11 for t in tuning_data['tuning']):break
    page.wait_for_timeout(500)
   else:raise AssertionError('tuning did not produce enough trials')
   page.wait_for_timeout(1200)
   page.get_by_label('Show individual tuning trials').wait_for()
   assert len(tune_requests)==1,'duplicate tuning request'
   assert evaluation_rows.count()==baseline,'tuning flooded ordinary evaluation rows'
   assert page.locator('.run-detail .panel-heading code').inner_text()==original
   assert page.evaluate('window._chartNode === document.querySelector(".run-detail svg")'),'completed chart rebuilt during tuning'
   page.get_by_label('Show individual tuning trials').check()
   page.wait_for_function('document.querySelectorAll(".evaluations tbody tr").length === 10')
   page.get_by_role('button',name='Next runs',exact=True).click()
   assert 'Page 2 of' in page.locator('.evaluation-pagination').inner_text()
   row=evaluation_rows.last
   selected_id=row.get_attribute('data-run-id')
   row.locator('td').nth(3).click()
   page.wait_for_function('(id) => document.querySelector(".run-detail .panel-heading code")?.textContent === id.slice(0,8)',arg=selected_id)
   # The name remains a keyboard-accessible button as well as row clicks.
   row.get_by_role('button').first.focus()
   page.keyboard.press('Enter')
   page.get_by_role('button',name='Tuning runs',exact=False).click()
   stop=page.get_by_role('button',name='Stop tuning',exact=True)
   if stop.count():
    stop.first.click()
   page.locator('.research-tabs button').first.click()
   page.get_by_label('Show individual tuning trials').uncheck()
   page.wait_for_function('(n) => document.querySelectorAll(".evaluations tbody tr").length === n',arg=baseline)
   page.get_by_role('button',name='Overview',exact=True).click()
   cash=page.locator('.evaluation-row').filter(has=page.get_by_role('button',name='Cash reference',exact=True)).first
   cash_id=cash.get_attribute('data-run-id')
   cash.locator('td').nth(3).click()
   page.wait_for_function('(id) => document.querySelector(".run-detail .panel-heading code")?.textContent === id.slice(0,8)',arg=cash_id)
   assert page.locator('.stats').count()==1
   assert page.locator('.controls').count()==0
   page.get_by_role('button',name='Backtests',exact=True).click()
   assert value[:8] in page.locator('.controls .strategy-control').inner_text(),'selection lost after navigating tabs'
   page.get_by_role('button',name='Leaderboard',exact=True).click()
   page.get_by_role('heading',name='Strategy leaderboard',exact=True).wait_for()
   assert page.locator('.stats,.controls,.run-detail').count()==0
   comparison=page.get_by_label('Comparison')
   comparison.locator('option').first.wait_for(state='attached')
   assert comparison.locator('option').count()>0
   rows=page.locator('.leaderboard tbody tr')
   rows.first.wait_for()
   scores=[float(t.replace('%','')) for t in rows.locator('td.numeric').all_text_contents()]
   assert scores==sorted(scores,reverse=True),'leaderboard is not sorted by objective score'
   assert rows.count()>0
   assert page.locator('.leaderboard th').nth(2).inner_text()=='Sharpe'
   choice=comparison.input_value()
   validation_strategy=next(g for g in submitted['leaderboards'] if g['period']=='development')['entries'][0]['strategy']
   validation_request=api_request(f'http://127.0.0.1:{port}/api/jobs',data=json.dumps({'strategy':validation_strategy,'campaign':submitted['jobs'][0]['campaign'],'period':'validation','idempotency_key':'browser-leaderboard-validation'}).encode())
   with urllib.request.urlopen(validation_request) as response:json.load(response)
   page.wait_for_function('Array.from(document.querySelectorAll(".leaderboard select option")).some(o => o.textContent.includes("validation"))')
   assert comparison.input_value()==choice,'comparison changed while new results arrived'
   validation_option=comparison.locator('option').filter(has_text='validation').first.get_attribute('value')
   comparison.select_option(validation_option)
   page.wait_for_function('Array.from(document.querySelectorAll(".leaderboard tbody td")).some(td => td.textContent === "Sharpe only")')
   assert comparison.input_value()==validation_option
   rows.first.get_by_role('button').click()
   page.locator('.run-detail h2').wait_for()
   assert page.get_by_role('button',name='View validation details',exact=True).count()==1
   with urllib.request.urlopen(api_request(f'http://127.0.0.1:{port}/api/overview')) as response:validation_data=json.load(response)
   assert not validation_data['validation']['revealed'],'leaderboard revealed validation'
   page.get_by_role('button',name='Leaderboard',exact=True).click()
   assert comparison.input_value()==validation_option,'comparison lost after navigation'
   comparison.select_option(choice)
   rows.first.get_by_role('button').click()
   page.get_by_role('heading',name='Backtests',exact=True).wait_for()
   page.locator('.run-detail h2').wait_for()
   page.get_by_role('button',name='History',exact=True).click()
   assert page.locator('.stats,.controls,.run-detail').count()==0
   page.get_by_role('heading',name='Strategy history book').wait_for()
   page.get_by_role('button',name='Paper trading',exact=True).click()
   page.get_by_role('heading',name='Paper portfolios').wait_for()
   assert page.locator('.stats,.controls,.run-detail').count()==0
   # Mock native provenance in this isolated UI fixture; these are not market results.
   manifest_path=next((state/'datasets').glob('*/manifest.json'))
   provenance=json.loads(manifest_path.read_text());provenance['synthetic']=False;provenance['venue']='bitvavo';manifest_path.write_text(json.dumps(provenance))
   page.wait_for_function('async () => (await (await fetch("/api/overview")).json()).paper_candidates.length > 0')
   with urllib.request.urlopen(api_request(f'http://127.0.0.1:{port}/api/overview')) as response:ready=json.load(response)
   strategy_id=next(s['id'] for s in ready['strategies'] if s['spec']['name']=='Trend explorer')
   page.locator('.paper-grid .picker-trigger').click()
   page.get_by_role('searchbox',name='Search strategies and versions').fill(strategy_id)
   page.locator('dialog[open] .version-option').click()
   start=page.get_by_role('button',name='Approve paper session',exact=True)
   start.click()
   session=page.locator('.paper-session').first
   session.wait_for()
   assert session.locator('.paper-activity').inner_text()
   assert 'Virtual equity' in session.inner_text()
   session.get_by_role('button',name='Stop session',exact=True).click()
   stopped=session.get_by_role('button',name='Stopped',exact=True)
   stopped.wait_for()
   assert stopped.is_disabled()
   with urllib.request.urlopen(api_request(f'http://127.0.0.1:{port}/api/overview')) as response:stopped_data=json.load(response)
   stopped_session=stopped_data['paper'][0]
   assert stopped_session['state']=='stopped'
   page.wait_for_timeout(1500)
   with urllib.request.urlopen(api_request(f'http://127.0.0.1:{port}/api/overview')) as response:after=json.load(response)
   assert after['paper'][0]['sequence']==stopped_session['sequence']
   page.locator(f'button[data-strategy-id="{strategy_id}"]').click()
   page.wait_for_function('document.querySelectorAll(".paper-session").length === 2')
   page.set_viewport_size({'width':390,'height':844})
   page.wait_for_function('document.querySelector(".sidebar").getBoundingClientRect().width === innerWidth')
   assert page.get_by_role('button',name='Leaderboard',exact=True).locator('.nav-text').is_visible()
   assert not page.get_by_role('button',name='Collapse sidebar',exact=True).is_visible()
   page.screenshot(path='/tmp/tradeclimb-mobile.png',full_page=True)
   assert page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'),'page overflows mobile viewport'
   page.get_by_role('button',name='Overview',exact=True).click()
   settings=page.locator('.account-menu>summary')
   settings.click()
   choices=page.locator('.theme-options')
   choices.get_by_role('button',name='Dark',exact=True).click()
   assert choices.get_by_role('button',name='Dark',exact=True).get_attribute('aria-pressed')=='true'
   assert page.locator('html').get_attribute('data-theme')=='dark'
   choices.get_by_role('button',name='System',exact=True).click()
   assert choices.get_by_role('button',name='System',exact=True).get_attribute('aria-pressed')=='true'
   assert page.locator('html').get_attribute('data-theme')=='system'
   page.keyboard.press('Escape')
   assert not page.locator('.account-menu').evaluate('el=>el.open')
   assert settings.evaluate('el=>el===document.activeElement')
   context=page.locator('.research-context-group')
   assert not context.evaluate('el=>el.open')
   context.locator('summary').click()
   assert context.evaluate('el=>el.open')
   assert 'excluded from development and validation' in context.inner_text()
   context.locator('summary').click()
   summary=page.locator('.summary-toggle')
   assert summary.get_attribute('aria-expanded')=='false'
   assert not page.locator('.stats').is_visible()
   summary.click()
   page.locator('.stats').wait_for()
   assert summary.get_attribute('aria-expanded')=='true'
   summary.click()
   filters=page.get_by_role('button',name='Filters ⌄',exact=True)
   assert filters.get_attribute('aria-expanded')=='false'
   assert not page.get_by_role('searchbox',name='Search runs').is_visible()
   filters.click()
   page.get_by_role('searchbox',name='Search runs').wait_for()
   assert page.get_by_role('button',name='Hide filters',exact=True).get_attribute('aria-expanded')=='true'
   page.get_by_role('button',name='Hide filters',exact=True).click()
   assert page.locator('.evaluation-row').first.locator('.mobile-detail-link').is_visible()
   assert page.locator('.evaluations').evaluate('el=>el.clientHeight>=el.scrollHeight-1'),'mobile panel clips its contents'
   assert page.evaluate('document.querySelector(".content").getBoundingClientRect().top>=document.querySelector(".topbar").getBoundingClientRect().bottom'),'header overlaps mobile scroll area'
   metric_columns=page.locator('.evaluation-row').evaluate_all('rows=>rows.map(row=>[...row.querySelectorAll("td.numeric")].map(cell=>{const r=cell.getBoundingClientRect();return {x:r.x,width:r.width}}))')
   for columns in metric_columns:
    assert len(columns)==3
    assert max(c['width'] for c in columns)-min(c['width'] for c in columns)<1
    assert all(columns[i+1]['x']-columns[i]['x']-columns[i]['width']>=8 for i in range(2))
    assert all(abs(columns[i]['x']-metric_columns[0][i]['x'])<1 for i in range(3))
   mobile_cash=page.locator(f'tr[data-run-id="{cash_id}"] .evaluation-meta')
   assert 'development' in mobile_cash.inner_text().lower()
   assert 'completed' in mobile_cash.inner_text().lower()
   page.set_viewport_size({'width':320,'height':740})
   assert page.evaluate('document.documentElement.scrollWidth <= innerWidth'),'page overflows narrow mobile viewport'
   empty=browser.new_page()
   empty.context.add_cookies(page.context.cookies())
   empty.on('pageerror',lambda e:errors.append(str(e)))
   empty.route('**/api/events',lambda route:route.abort())
   empty.route('**/api/overview',lambda route:route.fulfill(json={'strategies':[],'campaigns':[],'jobs':[],'leaderboards':[],'datasets':[],'paper':[],'validation':{}}))
   empty.goto(f'http://127.0.0.1:{port}',wait_until='domcontentloaded')
   empty.get_by_role('button',name='Leaderboard',exact=True).click()
   empty.get_by_role('heading',name='No ranked strategies yet',exact=True).wait_for()
   assert empty.get_by_label('Comparison').is_disabled()
   empty.get_by_role('button',name='Go to Backtests',exact=True).click()
   empty.get_by_role('heading',name='Backtests',exact=True).wait_for()
   empty.close()
   assert not errors,errors
   # Keep the actual browser's EventSource open during Ctrl+C.
   service.send_signal(signal.SIGINT)
   service.wait(timeout=8)
   assert service.returncode==0
   browser.close()
   print('PASS: clickable rows, grouped tuning trials, pagination, duplicate tuning protection, stable charts, sidebar, themes, leaderboard, paper controls, mobile layout, and Ctrl+C; no page errors')
 finally:
  service.send_signal(signal.SIGINT)
  try:service.wait(timeout=30)
  except subprocess.TimeoutExpired:service.kill();service.wait()
