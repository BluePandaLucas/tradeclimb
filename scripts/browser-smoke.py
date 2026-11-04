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
   page.wait_for_function('document.querySelector(".sidebar").getBoundingClientRect().width === 232')
   toggle=page.get_by_role('button',name='Toggle dark mode',exact=True)
   system=page.get_by_role('button',name='Use system theme',exact=True)
   toggle.wait_for()
   assert page.locator('html').get_attribute('data-theme')=='system'
   assert toggle.get_attribute('aria-pressed')=='true'
   assert page.evaluate('getComputedStyle(document.body).backgroundColor') in ('rgba(0, 0, 0, 0)','rgb(17, 26, 30)')
   assert page.evaluate('getComputedStyle(document.documentElement).backgroundColor')=='rgb(17, 26, 30)'
   assert '0.22s' in page.locator('html').evaluate('el => getComputedStyle(el).transitionDuration')
   page.emulate_media(reduced_motion='reduce')
   assert page.locator('html').evaluate('el => getComputedStyle(el).transitionDuration')=='0s'
   page.emulate_media(reduced_motion='no-preference')
   toggle.click()
   assert page.locator('html').get_attribute('data-theme')=='light'
   page.reload(wait_until='domcontentloaded')
   toggle.wait_for()
   assert page.locator('html').get_attribute('data-theme')=='light'
   system.click()
   page.emulate_media(color_scheme='light')
   page.wait_for_function('getComputedStyle(document.documentElement).backgroundColor === "rgb(246, 248, 250)"')
   page.emulate_media(color_scheme='dark')
   page.wait_for_function('getComputedStyle(document.documentElement).backgroundColor === "rgb(17, 26, 30)"')
   page.get_by_role('button',name='Trend explorer',exact=True).first.wait_for()
   page.get_by_role('button',name='Backtests',exact=True).click()
   assert page.locator('.stats').count()==0
   assert page.locator('.controls').count()==1
   selects=page.locator('.controls select')
   initial=selects.nth(0).input_value()
   subprocess.run([str(root/'target/release/tc'),'--data-dir',folder,'builtin','--name','New fixture version'],check=True,capture_output=True)
   page.wait_for_timeout(2200)
   assert selects.nth(0).input_value()==initial,'default selection changed while new versions arrived'
   page.get_by_role('button',name='＋ Run backtest',exact=True).click()
   page.wait_for_timeout(1500)
   with urllib.request.urlopen(api_request(f'http://127.0.0.1:{port}/api/overview')) as response:submitted=json.load(response)
   assert submitted['jobs'][0]['strategy']==initial,'submitted strategy differs from visible selection'
   value=next(option.get_attribute('value') for option in selects.nth(0).locator('option').all() if option.get_attribute('value')!=initial)
   selects.nth(0).select_option(value)
   page.wait_for_timeout(2200)
   assert selects.nth(0).input_value()==value,'strategy selection reset after stream update'
   page.get_by_role('button',name='Trend explorer',exact=True).first.click()
   page.get_by_role('img',name='Net portfolio equity').wait_for()
   page.get_by_label('Chart window').select_option('7')
   page.wait_for_timeout(2200)
   assert page.get_by_label('Chart window').input_value()=='7'
   page.get_by_label('BTC reference',exact=True).uncheck()
   page.wait_for_timeout(1200)
   assert not page.get_by_label('BTC reference',exact=True).is_checked()
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
   page.wait_for_function('async () => { const d=await (await fetch("/api/overview")).json(); return d.tuning.some(t => (t.jobs || []).length >= 11); }',timeout=60000)
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
   stop=page.get_by_role('button',name='Stop tuning',exact=True)
   if stop.count():
    stop.first.click()
   page.get_by_label('Show individual tuning trials').uncheck()
   assert evaluation_rows.count()==baseline
   page.get_by_role('button',name='Overview',exact=True).click()
   cash=page.locator('.evaluation-row').filter(has=page.get_by_role('button',name='Cash reference',exact=True)).first
   cash_id=cash.get_attribute('data-run-id')
   cash.locator('td').nth(3).click()
   page.wait_for_function('(id) => document.querySelector(".run-detail .panel-heading code")?.textContent === id.slice(0,8)',arg=cash_id)
   assert page.locator('.stats').count()==1
   assert page.locator('.controls').count()==0
   page.get_by_role('button',name='Backtests',exact=True).click()
   assert selects.nth(0).input_value()==value,'selection lost after navigating tabs'
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
   page.get_by_role('heading',name='Run details',exact=True).wait_for()
   assert page.get_by_text('Validation detail is withheld.',exact=False).count()==1
   with urllib.request.urlopen(api_request(f'http://127.0.0.1:{port}/api/overview')) as response:validation_data=json.load(response)
   assert not validation_data['validation']['revealed'],'leaderboard revealed validation'
   page.get_by_role('button',name='Leaderboard',exact=True).click()
   assert comparison.input_value()==validation_option,'comparison lost after navigation'
   comparison.select_option(choice)
   rows.first.get_by_role('button').click()
   page.get_by_role('heading',name='Backtests',exact=True).wait_for()
   page.get_by_role('heading',name='Run details',exact=True).wait_for()
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
   start=page.get_by_role('button',name='Approve Trend explorer',exact=True)
   strategy_id=start.get_attribute('data-strategy-id')
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
   page.get_by_role('button',name='Collapse sidebar',exact=True).click()
   page.wait_for_function('document.querySelector(".sidebar").getBoundingClientRect().width === 64')
   assert not page.get_by_role('button',name='Leaderboard',exact=True).locator('.nav-text').is_visible()
   page.get_by_role('button',name='Expand sidebar',exact=True).click()
   assert page.get_by_role('button',name='Leaderboard',exact=True).locator('.nav-text').is_visible()
   page.get_by_role('button',name='Collapse sidebar',exact=True).click()
   page.screenshot(path='/tmp/tradeclimb-mobile.png',full_page=True)
   assert page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'),'page overflows mobile viewport'
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
