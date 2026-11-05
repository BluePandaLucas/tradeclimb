#!/usr/bin/env python3
"""Stress the real WASM UI against a synthetic, growing research library."""
import json
import pathlib
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.request
import uuid
from playwright.sync_api import sync_playwright, expect

root = pathlib.Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='tc-backlog-') as folder:
    state = pathlib.Path(folder)
    subprocess.run(['python3', str(root/'scripts/ui-demo.py'), '--data-dir', folder], check=True, capture_output=True)
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    owner_key=(state/'auth/owner.key').read_text().strip()
    def api_request(url):return urllib.request.Request(url,headers={'Authorization':'Bearer '+owner_key})
    service = subprocess.Popen([str(root/'target/release/tc'), '--data-dir', folder, 'serve', '--bind', f'127.0.0.1:{port}', '--assets', str(root/'dist')], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(100):
            try:
                with urllib.request.urlopen(api_request(f'http://127.0.0.1:{port}/api/overview')) as response:
                    data = json.load(response)
                break
            except OSError:
                time.sleep(.1)
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=['--no-sandbox'])
            page = browser.new_page(viewport={'width':1440,'height':1000}, color_scheme='dark')
            errors = []
            page.on('pageerror', lambda e:errors.append(str(e)))
            page.goto(f'http://127.0.0.1:{port}/login')
            page.get_by_label('Owner key').fill(owner_key)
            page.get_by_role('button',name='Sign in',exact=True).click()
            page.wait_for_function('document.querySelector(".evaluation-pagination")?.textContent.includes("2003 runs")')
            assert page.locator('.evaluation-row').count()==10
            assert page.locator('.run-detail').count()==0, 'arrival opened an unsolicited chart'
            page.get_by_role('button',name='Backtests',exact=True).click()
            trigger=page.locator('.controls .picker-trigger')
            trigger.click()
            dialog=page.locator('dialog[open]')
            expect(dialog.get_by_role('searchbox')).not_to_be_focused()
            while not dialog.get_by_role('button',name='Previous strategies').is_disabled():
                dialog.get_by_role('button',name='Previous strategies').click()
            expect(dialog.locator('.family-option')).to_have_count(30)
            assert dialog.locator('.version-option').count()<=30
            assert not dialog.get_by_role('button',name='Next strategies').is_disabled()
            dialog.get_by_role('button',name='Next strategies').click()
            dialog.get_by_role('button',name='Previous strategies').click()
            search=dialog.get_by_role('searchbox')
            search_start=time.perf_counter()
            search.fill('Adaptive trend 001')
            expect(dialog.locator('.family-option')).to_have_count(1)
            search_ms=(time.perf_counter()-search_start)*1000
            expect(dialog.locator('.version-option')).to_have_count(30)
            dialog.get_by_role('button',name='Next versions').click()
            expect(dialog.locator('.version-option')).to_have_count(9)
            selected=dialog.locator('.version-option').last
            version=selected.locator('small').inner_text().split(' · ')[0]
            selected.click()
            assert version in trigger.inner_text()
            trigger.focus()
            page.keyboard.press('Enter')
            assert dialog.is_visible()
            expect(dialog.get_by_role('searchbox')).not_to_be_focused()
            page.keyboard.press('Tab')
            expect(dialog.get_by_role('searchbox')).to_be_focused()
            # Growing the library must not close the picker or reset its search.
            search.fill('Adaptive trend 001')
            db=sqlite3.connect(state/'tradeclimb.db')
            strategy=json.loads(db.execute("SELECT data FROM objects WHERE kind='strategy' LIMIT 1").fetchone()[0])
            strategy.update(id=uuid.uuid4().hex,parent=strategy['id'])
            db.execute('INSERT INTO objects VALUES(?,?,?)',('strategy',strategy['id'],json.dumps(strategy)))
            db.commit()
            page.wait_for_timeout(2200)
            assert dialog.is_visible() and search.input_value()=='Adaptive trend 001'
            page.keyboard.press('Escape')
            expect(trigger).to_be_focused()
            assert version in trigger.inner_text()
            # Older runs are searchable beyond both former 50/500 caps.
            old=json.loads(db.execute('SELECT data FROM jobs ORDER BY rowid ASC LIMIT 1 OFFSET 3').fetchone()[0])
            page.get_by_role('searchbox',name='Search runs').fill(old['id'])
            page.wait_for_function('document.querySelectorAll(".evaluation-row").length===1')
            assert page.locator('.evaluation-row').get_attribute('data-run-id')==old['id']
            page.get_by_role('searchbox',name='Search runs').fill('')
            page.get_by_role('button',name='Next runs',exact=True).click()
            before=page.locator('.evaluation-row').evaluate_all('els=>els.map(el=>el.dataset.runId)')
            job=json.loads(db.execute('SELECT data FROM jobs ORDER BY rowid DESC LIMIT 1').fetchone()[0])
            job.update(id=str(uuid.uuid4()),created_at=int(time.time()*1000),updated_at=int(time.time()*1000))
            db.execute('INSERT INTO jobs VALUES(?,?,?,?)',(job['id'],job['id'],job['id'],json.dumps(job)))
            db.commit()
            page.get_by_role('button',name='Show 1 new runs',exact=True).wait_for()
            assert page.locator('.evaluation-row').evaluate_all('els=>els.map(el=>el.dataset.runId)')==before
            page.get_by_role('button',name='Show 1 new runs',exact=True).click()
            assert page.locator('.evaluation-row').first.get_attribute('data-run-id')==job['id']
            page.locator('.research-tabs button').nth(1).click()
            expect(page.locator('.tuning-session')).to_have_count(5)
            page.get_by_role('button',name='Next tuning',exact=True).click()
            assert 'Page 2 of 48' in page.locator('.evaluation-pagination').inner_text()
            assert '<' not in page.locator('.evaluation-pagination').inner_text()
            page.get_by_role('button',name='History',exact=True).click()
            expect(page.locator('.history-entry')).to_have_count(1)
            archived=json.loads(db.execute("SELECT data FROM objects WHERE kind='strategy' AND json_extract(data,'$.archived')=1 LIMIT 1").fetchone()[0])
            page.locator('.history-browser .picker-trigger').click()
            dialog.get_by_role('searchbox').fill(archived['id'])
            dialog.locator('.version-option').click()
            page.get_by_role('button',name='Restore',exact=True).wait_for()
            page.wait_for_timeout(1200)
            expect(page.get_by_role('button',name='Restore',exact=True)).to_be_visible()
            page.get_by_role('button',name='Restore',exact=True).click()
            page.get_by_role('button',name='Archive',exact=True).wait_for()
            page.get_by_role('button',name='Paper trading',exact=True).click()
            expect(page.locator('.paper-grid>button, .paper-grid .picker-trigger')).to_have_count(2)
            page.get_by_role('button',name='Backtests',exact=True).click()
            page.set_viewport_size({'width':390,'height':844})
            page.wait_for_function('document.querySelector(".sidebar").getBoundingClientRect().width === innerWidth')
            assert page.evaluate('document.documentElement.scrollWidth<=innerWidth')
            trigger.click()
            assert dialog.evaluate('el=>el.getBoundingClientRect().right<=innerWidth')
            assert dialog.locator('section[aria-label="Versions"]').is_visible()
            search.fill('no-such-strategy')
            expect(dialog.get_by_text('No matching strategies.',exact=False)).to_be_visible()
            page.keyboard.press('Escape')
            page.emulate_media(color_scheme='light')
            trigger.click()
            assert dialog.is_visible()
            assert not errors, errors
            print(f'Strategy search completed in {search_ms:.0f} ms (browser round trip).')
            print('Backlog browser checks passed: 4,803 versions, 2,003 backtests, 240 tuning runs; live arrivals, old-run search, keyboard, mobile, light/dark.')
            browser.close()
    finally:
        service.terminate()
        service.wait(timeout=15)
