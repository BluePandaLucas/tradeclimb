#!/usr/bin/env python3
"""Optional headless browser checks. Requires Playwright and its Chromium runtime."""
import json,pathlib,signal,socket,subprocess,tempfile,time,urllib.request
from playwright.sync_api import sync_playwright
root=pathlib.Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='tc-ui-') as folder:
 state=pathlib.Path(folder)
 subprocess.run([str(root/'target/release/tc'),'--data-dir',folder,'demo'],check=True,capture_output=True)
 with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
 service=subprocess.Popen([str(root/'target/release/tc'),'--data-dir',folder,'serve','--bind',f'127.0.0.1:{port}','--assets',str(root/'dist')],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 try:
  for _ in range(100):
   try:
    with urllib.request.urlopen(f'http://127.0.0.1:{port}/api/overview') as response:data=json.load(response)
    if all(j['state']=='completed' for j in data['jobs']):break
   except OSError:pass
   time.sleep(.2)
  with sync_playwright() as p:
   browser=p.chromium.launch(headless=True,args=['--no-sandbox'])
   page=browser.new_page(viewport={'width':1440,'height':1000},color_scheme="dark")
   errors=[]
   page.on('pageerror',lambda e:errors.append(str(e)))
   page.goto(f'http://127.0.0.1:{port}',wait_until='domcontentloaded')
   page.get_by_role('heading',name='Overview',exact=True).wait_for()
   toggle=page.get_by_role('button',name='Toggle dark mode',exact=True)
   system=page.get_by_role('button',name='Use system theme',exact=True)
   toggle.wait_for()
   assert page.locator('html').get_attribute('data-theme')=='system'
   assert toggle.get_attribute('aria-pressed')=='true'
   assert page.evaluate('getComputedStyle(document.body).backgroundColor') in ('rgba(0, 0, 0, 0)','rgb(17, 26, 30)')
   assert page.evaluate('getComputedStyle(document.documentElement).backgroundColor')=='rgb(17, 26, 30)'
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
   selects=page.locator('select')
   initial=selects.nth(0).input_value()
   subprocess.run([str(root/'target/release/tc'),'--data-dir',folder,'builtin','--name','New fixture version'],check=True,capture_output=True)
   page.wait_for_timeout(2200)
   assert selects.nth(0).input_value()==initial,'default selection changed while new versions arrived'
   page.get_by_role('button',name='＋ Run backtest',exact=True).click()
   page.wait_for_timeout(1500)
   with urllib.request.urlopen(f'http://127.0.0.1:{port}/api/overview') as response:submitted=json.load(response)
   assert submitted['jobs'][0]['strategy']==initial,'submitted strategy differs from visible selection'
   value=selects.nth(0).locator('option').nth(1).get_attribute('value')
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
   page.get_by_role('button',name='History',exact=True).click()
   page.get_by_role('heading',name='Strategy history book').wait_for()
   page.get_by_role('button',name='Paper trading',exact=True).click()
   page.get_by_role('heading',name='Paper portfolios').wait_for()
   start=page.get_by_role('button',name='Start Trend explorer',exact=True)
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
   with urllib.request.urlopen(f'http://127.0.0.1:{port}/api/overview') as response:stopped_data=json.load(response)
   stopped_session=stopped_data['paper'][0]
   assert stopped_session['state']=='stopped'
   page.wait_for_timeout(1500)
   with urllib.request.urlopen(f'http://127.0.0.1:{port}/api/overview') as response:after=json.load(response)
   assert after['paper'][0]['sequence']==stopped_session['sequence']
   page.locator(f'button[data-strategy-id="{strategy_id}"]').click()
   page.wait_for_function('document.querySelectorAll(".paper-session").length === 2')
   page.set_viewport_size({'width':390,'height':844})
   page.screenshot(path='/tmp/tradeclimb-mobile.png',full_page=True)
   assert page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'),'page overflows mobile viewport'
   assert not errors,errors
   # Keep the actual browser's EventSource open during Ctrl+C.
   service.send_signal(signal.SIGINT)
   service.wait(timeout=8)
   assert service.returncode==0
   browser.close()
   print('PASS: system/dark/light themes, persistence, paper startup/stop controls, charts, mobile layout, and Ctrl+C with browser connected; no page errors')
 finally:
  service.send_signal(signal.SIGINT)
  try:service.wait(timeout=30)
  except subprocess.TimeoutExpired:service.kill();service.wait()
