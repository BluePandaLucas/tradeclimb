#!/usr/bin/env python3
"""Build hostile build.rs offline, then test scoped inference inside real workers."""
import json
import os
import pathlib
import shutil
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
BINARY = ROOT / 'target/release/tc'
with tempfile.TemporaryDirectory(prefix='tc-sandbox-smoke-') as directory:
    state = pathlib.Path(directory)
    data = state / 'data'
    def cli(*args, **kwargs):
        result = subprocess.run([str(BINARY), '--data-dir', str(data), *map(str,args)], cwd=ROOT, text=True, capture_output=True, **kwargs)
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)
    assert cli('sandbox-check')['ready']
    source = state / 'source'
    shutil.copytree(ROOT / 'examples/strategy', source, ignore=shutil.ignore_patterns('target', '.git'))
    private = state / 'private-validation'
    private.write_text('owner-only')
    (source / '.env').write_text('owner-only')
    with socket.socket() as listener:
        listener.bind(('127.0.0.1',0))
        listener.listen()
        host_port = listener.getsockname()[1]
        (source / 'build.rs').write_text('''fn main() {
assert!(std::fs::read_to_string(PRIVATE).is_err());
assert!(std::fs::read_to_string(".env").is_err());
assert!(std::env::var("TC_OWNER_BUILD_SECRET").is_err());
assert!(std::net::TcpStream::connect_timeout(&"127.0.0.1:PORT".parse().unwrap(), std::time::Duration::from_millis(100)).is_err());
assert!(std::fs::write("/host-write", "bad").is_err());
println!("cargo:rustc-env=TC_BUILD_ISOLATED=yes");
}'''.replace('PRIVATE',json.dumps(str(private))).replace('PORT',str(host_port)))
        (source / 'src/main.rs').write_text('''use tc_sdk::{Strategy, Frame, Intent, ModelClient};
struct Fixture(bool);
impl Strategy for Fixture {
 fn on_frame(&mut self, _: &Frame) -> anyhow::Result<Vec<Intent>> {
  assert_eq!(env!("TC_BUILD_ISOLATED"),"yes");
  if !self.0 {
   let mut client=tc_sdk::HttpModelClient::from_env()?;
   let result=client.infer(&tc_sdk::ModelRequest {capability:"fake".into(),prompt:"scoped sandbox model".into(),max_tokens:10})?;
   anyhow::ensure!(result.text=="scoped sandbox model");
   self.0=true;
  }
  Ok(vec![])
 }
}
fn main()->anyhow::Result<()> {tc_sdk::serve(Fixture(false))}
''')
        artifact = state / 'compiled-strategy'
        build = cli('build','--project',source,'--sdk-root',ROOT,'--bin','tc-example-strategy','--output',artifact,env=dict(os.environ,TC_OWNER_BUILD_SECRET='owner-only'))
        assert build['offline'] and build['bytes'] == artifact.stat().st_size
    assert private.read_text() == 'owner-only'
    # The build lockfile is mandatory; there is no unsandboxed retry path.
    lockfile = (source / 'Cargo.lock').read_bytes()
    (source / 'Cargo.lock').unlink()
    failure = subprocess.run([str(BINARY),'--data-dir',str(data),'build','--project',str(source),'--sdk-root',str(ROOT),'--bin','tc-example-strategy','--output',str(state/'must-not-exist')],capture_output=True)
    assert failure.returncode != 0 and not (state/'must-not-exist').exists()
    (source / 'Cargo.lock').write_bytes(lockfile)
    demo = cli('demo')
    strategy = cli('register','--name','Confined model fixture','--description','Sandbox integration test','--project',source,'--executable',artifact)
    with socket.socket() as listener:
        listener.bind(('127.0.0.1',0)); port=listener.getsockname()[1]
    base=f'http://127.0.0.1:{port}'
    def request(path,body=None):
        req=urllib.request.Request(base+path,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'})
        with urllib.request.urlopen(req,timeout=10) as response:return json.load(response)
    def until(fn):
        deadline=time.monotonic()+120
        while time.monotonic()<deadline:
            try:
                result=fn()
                if result:return result
            except OSError:pass
            time.sleep(.1)
        raise AssertionError('sandbox worker timed out')
    with open(state/'service.log','w+') as log:
        service=subprocess.Popen([str(BINARY),'--data-dir',str(data),'serve','--bind',f'127.0.0.1:{port}','--assets',str(ROOT/'dist')],stdout=log,stderr=log,env=dict(os.environ,TC_MODEL_MAP='{}'))
        try:
            until(lambda:request('/api/overview'))
            jobs=[]
            for period in ['development','validation']:
                job=request('/api/jobs',{'strategy':strategy['id'],'campaign':demo['campaign'],'period':period,'idempotency_key':'sandbox-'+period})
                def done():
                    value=request('/api/jobs/'+job['id'])
                    assert value['state'] not in ('failed','interrupted'),value
                    return value if value['state']=='completed' else None
                result=until(done)
                if period=='validation':assert set(result['result'])=={'sharpe'}
                jobs.append(job['id'])
            with sqlite3.connect(data/'tradeclimb.db') as db:
                rows=db.execute('SELECT scope,COUNT(*) FROM model_cache GROUP BY scope').fetchall()
                assert sorted(rows)==sorted((job,1) for job in jobs),rows
        finally:
            service.send_signal(signal.SIGINT)
            service.wait(timeout=8)
            assert service.returncode==0
print('PASS: confined offline build.rs, hidden files/environment/network, frozen lockfile, scoped models, development/validation workers, and shutdown')
