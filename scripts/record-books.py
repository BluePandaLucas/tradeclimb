#!/usr/bin/env python3
"""Bounded, read-only forward Bitvavo depth recorder. Never submits orders."""
import argparse,hashlib,json,os,pathlib,time,urllib.request
MARKETS=['BTC-EUR','ETH-EUR','SOL-EUR','XRP-EUR','ADA-EUR']
BASE='https://api.bitvavo.com/v2'
DOC='https://docs.bitvavo.com/docs/rest-api/get-order-book/'
def record(destination,seconds=3600,interval=5,max_bytes=1024*1024*1024):
 if not 1<=seconds<=86400 or interval<1 or max_bytes<1024:raise ValueError('Use 1–86400 seconds, interval >=1 second and a positive bounded byte budget')
 destination=pathlib.Path(destination);destination.mkdir(mode=0o700,parents=True,exist_ok=False)
 archive=destination/'events.jsonl';digest=hashlib.sha256();size=0;events=0;errors=0;previous={};reason='duration';started=time.monotonic();start_utc=time.time_ns()
 # Each response is an independent snapshot. A nonce jump is expected between polls;
 # no interpolation or continuous reconstructed order book is claimed.
 with archive.open('xb') as output:
  try:
   while time.monotonic()-started<seconds:
    cycle=time.monotonic()
    for market in MARKETS:
     if time.monotonic()-started>=seconds:break
     sent=time.time_ns();sent_mono=time.monotonic_ns();event={'market':market,'sent_utc_ns':sent}
     try:
      request=urllib.request.Request(f'{BASE}/{market}/book?depth=1000',headers={'User-Agent':'TradeClimb/0.2 public-data-research'})
      with urllib.request.urlopen(request,timeout=5) as response:
       raw=response.read(2*1024*1024+1)
       if len(raw)>2*1024*1024:raise ValueError('Book response exceeded byte limit')
       remaining=response.headers.get('bitvavo-ratelimit-remaining');reset=response.headers.get('bitvavo-ratelimit-resetat')
      received=time.time_ns();rtt=time.monotonic_ns()-sent_mono
      book=json.loads(raw);nonce=book.get('nonce')
      if book.get('market')!=market or not isinstance(nonce,int) or not isinstance(book.get('timestamp'),int):raise ValueError('Book identity/nonce/timestamp missing')
      # Preserve original JSON bytes, server nanosecond time and nonce for later replay.
      event.update({'received_utc_ns':received,'request_duration_ns':rtt,'snapshot_timestamp_ns':book['timestamp'],'nonce':nonce,'previous_polled_nonce':previous.get(market),'raw_json':raw.decode('utf-8'),'response_sha256':hashlib.sha256(raw).hexdigest()})
      previous[market]=nonce
      if remaining is not None and int(remaining)<30:
       event['rate_limit_pause_until_ms']=int(reset or (time.time()*1000+60000))
     except Exception as error:
      errors+=1;event.update({'received_utc_ns':time.time_ns(),'request_duration_ns':time.monotonic_ns()-sent_mono,'error':str(error)})
     line=(json.dumps(event,separators=(',',':'))+'\n').encode()
     if size+len(line)>max_bytes:reason='byte_budget';break
     output.write(line);output.flush();digest.update(line);size+=len(line);events+=1
     if 'rate_limit_pause_until_ms' in event:
      pause=max(0,event['rate_limit_pause_until_ms']/1000-time.time());time.sleep(min(pause,max(0,seconds-(time.monotonic()-started))))
    if reason=='byte_budget':break
    time.sleep(min(max(0,interval-(time.monotonic()-cycle)),max(0,seconds-(time.monotonic()-started))))
  except KeyboardInterrupt:reason='owner_stop'
  finally:
   output.flush();os.fsync(output.fileno())
   manifest={'version':'book_snapshots_v1','venue':'bitvavo','started_utc_ns':start_utc,'finished_utc_ns':time.time_ns(),'requested_seconds':seconds,'interval_seconds':interval,'events':events,'errors':errors,'bytes':size,'stop_reason':reason,'sha256':digest.hexdigest(),'file':archive.name,'source':DOC,'limits':['Independent REST snapshots, not a complete event stream or queue reconstruction','Server timestamp is nanoseconds; local clock skew and request duration are retained, not corrected','Unobserved intervals, failures and cancelled liquidity must remain unknown','No exchange orders, inferred fills or execution-cost calibration are produced by recording']}
   (destination/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
 return manifest
if __name__=='__main__':
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('destination');parser.add_argument('--seconds',type=int,default=3600);parser.add_argument('--interval',type=float,default=5);parser.add_argument('--max-bytes',type=int,default=1024*1024*1024)
 args=parser.parse_args();print(json.dumps(record(args.destination,args.seconds,args.interval,args.max_bytes),indent=2))
