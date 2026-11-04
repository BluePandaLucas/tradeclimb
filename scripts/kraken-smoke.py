#!/usr/bin/env python3
"""Adversarial byte-range/ZIP fixture; no exchange access or historical market prices."""
import importlib.util,io,http.server,json,pathlib,socketserver,tempfile,threading,zipfile
ROOT=pathlib.Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('kraken_download',ROOT/'scripts/kraken-download.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
payload=io.BytesIO()
with zipfile.ZipFile(payload,'w',compression=zipfile.ZIP_DEFLATED) as archive:
 archive.writestr('fixture.csv','generated control fixture\n'*100000)
raw=payload.getvalue();cut=len(raw)//2;parts={'/first':raw[:cut],'/second':raw[cut:]}
class Handler(http.server.BaseHTTPRequestHandler):
 corrupt=False
 def do_GET(self):
  body=parts[self.path];start,end=map(int,self.headers['Range'].removeprefix('bytes=').split('-'))
  assert 0<=start<=end<len(body)
  self.send_response(206);self.send_header('Content-Range',f'bytes {start}-{end}/{len(body)}')
  self.send_header('ETag','"changed"' if Handler.corrupt and self.headers.get('If-Match') else '"fixed"');self.end_headers();self.wfile.write(body[start:end+1])
 def log_message(self,*args):pass
with socketserver.TCPServer(('127.0.0.1',0),Handler) as server:
 thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
 base=f'http://127.0.0.1:{server.server_address[1]}'
 remote=module.RemoteParts([base+'/first',base+'/second'])
 with zipfile.ZipFile(remote) as archive:assert archive.read('fixture.csv')==b'generated control fixture\n'*100000
 changed=module.RemoteParts([base+'/first',base+'/second']);Handler.corrupt=True
 try:changed.read(20);raise AssertionError('changed ETag accepted')
 except ValueError as error:assert 'changed' in str(error)
 with tempfile.TemporaryDirectory() as folder:
  try:module.download(pathlib.Path(folder)/'excluded',module.millis('2026-08-01'),module.millis('2026-11-01'));raise AssertionError('reserved research download accepted')
  except ValueError:pass
  assert not (pathlib.Path(folder)/'excluded').exists()
 server.shutdown();thread.join()
print('PASS: cross-part ZIP reads, CRC, byte ranges, changing archive rejection and pre-holdout download gate')
