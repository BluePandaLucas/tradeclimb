#!/usr/bin/env python3
"""Exercise chronological selection and final-test controls using temporary synthetic fixtures."""
import json,pathlib,sqlite3,subprocess,tempfile
ROOT=pathlib.Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='tc-evaluation-') as folder:
 data=pathlib.Path(folder)/'data'
 def cli(*args,ok=True):
  result=subprocess.run([str(ROOT/'target/release/tc'),'--data-dir',str(data),*args],capture_output=True,text=True)
  assert (result.returncode==0)==ok,result.stderr
  return json.loads(result.stdout) if ok else result.stderr
 demo=cli('demo');versions=[s['strategy'] for s in demo['strategies']]
 before=cli('budget')
 wf=cli('walk-forward','--campaign',demo['campaign'],'--candidates',*versions,'--folds','2')
 assert wf['state']=='completed' and len(wf['results'])==2
 assert cli('walk-forward-report',wf['id'])==wf
 campaign=cli('list')['campaigns'][0]
 for fold in wf['results']:
  assert fold['train_end']==fold['test_start']
  assert fold['test_end']<=campaign['development_end']
  # Independently verify that only eligible training results determined selection.
  choices=[r for r in fold['training'] if r['metrics']['eligible'] and r['metrics']['sharpe'] is not None]
  if choices:
   winner=sorted(choices,key=lambda r:(-r['metrics']['sharpe'],r['strategy']))[0]
   assert fold['test']['strategy']==winner['strategy']
  else:assert 'skipped' in fold['test']
 assert cli('budget')==before
 assert cli('final-test','status')=={'state':'sealed','consumed':False}
 assert 'explicit --consume-final-test' in cli('final-test','execute','missing',ok=False)
 # Synthetic campaign cannot provide an eligible native holdout candidate.
 assert cli('final-test','prepare',demo['strategies'][0]['job'],ok=False)
 # Deliberately mock native provenance/eligibility in this temporary fixture to
 # exercise plan preparation. No real holdout observations exist or are run.
 manifest_path=data/'datasets'/demo['dataset']/'manifest.json'
 manifest=json.loads(manifest_path.read_text());manifest.update(synthetic=False,venue='bitvavo',accepted=True,end=1793491200000)
 manifest_path.write_text(json.dumps(manifest))
 frozen_campaign=cli('campaign',demo['dataset'])
 with sqlite3.connect(data/'tradeclimb.db') as database:
  job_id=demo['strategies'][0]['job']
  job=json.loads(database.execute('select data from jobs where id=?',(job_id,)).fetchone()[0])
  job.update(state='completed',campaign=frozen_campaign['id'],result={'metrics':{'eligible':True,'sharpe':1.2,'net_cagr':0.10,'max_drawdown':0.10}})
  database.execute('update jobs set data=? where id=?',(json.dumps(job),job_id))
 plan=cli('final-test','prepare',job_id)
 assert plan['development_job']==job_id and plan['config']['start']==1785542400000 and plan['config']['end']==1793491200000
 assert plan['dataset_manifest_hash'] and plan['engine_build_hash']
 assert 'explicit --consume-final-test' in cli('final-test','execute',plan['id'],ok=False)
 assert cli('final-test','status')['state']=='sealed'
 d=cli('diagnostics');assert d['walk_forward_evaluation_attempts']>=6
 assert d['final_test']['state']=='sealed' and d['validation']['used']==0
 print('PASS: development-only chronological selection, diagnostic counts, unchanged validation budget and sealed final-test controls')
