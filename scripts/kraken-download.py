#!/usr/bin/env python3
"""Fetch only five EUR one-minute files from Kraken's official ZIP using HTTP ranges."""
import argparse,collections,datetime,hashlib,io,json,os,pathlib,re,sys,time,urllib.request,zipfile
BASE='https://assets.kraken.com/marketing/institutions/'
PAGE='https://support.kraken.com/articles/360047124832-downloadable-historical-ohlcvt-open-high-low-close-volume-trades-data'
PAIRS={'BTC-EUR':('XBTEUR','XXBTZEUR','BTCEUR'),'ETH-EUR':('ETHEUR','XETHZEUR'),'SOL-EUR':('SOLEUR',),'XRP-EUR':('XRPEUR','XXRPZEUR'),'ADA-EUR':('ADAEUR',)}
class RemoteParts(io.RawIOBase):
 def __init__(self,urls):
  self.parts=[];self.position=0;self.length=0;self.cache=collections.OrderedDict();self.downloaded=0
  for url in urls:
   with self.request(url,0,0) as response:
    match=re.fullmatch(r'bytes 0-0/(\d+)',response.headers.get('Content-Range',''))
    if response.status!=206 or not match:raise ValueError('Server must support byte ranges: '+url)
    size=int(match[1]);etag=response.headers.get('ETag')
    if not etag or etag.startswith('W/'):raise ValueError('Strong ETag required for stable archive: '+url)
    self.parts.append({'url':url,'start':self.length,'size':size,'etag':etag});self.length+=size
 def request(self,url,start,end,etag=None):
  headers={'User-Agent':'Mozilla/5.0 TradeClimb/0.2 public-data-research','Range':f'bytes={start}-{end}','Accept-Encoding':'identity'}
  if etag:headers['If-Match']=etag
  for attempt in range(3):
   try:return urllib.request.urlopen(urllib.request.Request(url,headers=headers),timeout=30)
   except OSError:
    if attempt==2:raise
    time.sleep(2**attempt)
 def readable(self):return True
 def seekable(self):return True
 def tell(self):return self.position
 def seek(self,offset,whence=0):
  target=offset if whence==0 else self.position+offset if whence==1 else self.length+offset if whence==2 else -1
  if target<0:raise ValueError('invalid archive seek')
  self.position=target;return target
 def read(self,size=-1):
  end=self.length if size<0 else min(self.length,self.position+size);chunks=[]
  while self.position<end:
   part=next(p for p in self.parts if p['start']<=self.position<p['start']+p['size'])
   local=self.position-part['start'];block=local//(1024*1024);key=(part['url'],block)
   if key not in self.cache:
    start=block*1024*1024;finish=min(part['size'],start+1024*1024)-1
    with self.request(part['url'],start,finish,part['etag']) as response:
     expected=f'bytes {start}-{finish}/{part["size"]}'
     if response.status!=206 or response.headers.get('Content-Range')!=expected or response.headers.get('ETag')!=part['etag']:
      raise ValueError('Archive changed or invalid byte-range response')
     payload=response.read(finish-start+2)
     if len(payload)!=finish-start+1:raise ValueError('Truncated/oversized archive range')
    self.cache[key]=payload;self.downloaded+=len(payload)
    while len(self.cache)>8:self.cache.popitem(last=False)
   block_data=self.cache[key];self.cache.move_to_end(key)
   offset=local-block*1024*1024;count=min(len(block_data)-offset,end-self.position)
   chunks.append(block_data[offset:offset+count]);self.position+=count
  return b''.join(chunks)
def millis(date):return int(datetime.datetime.strptime(date,'%Y-%m-%d').replace(tzinfo=datetime.timezone.utc).timestamp()*1000)
def download(destination,start,end,quarter=None):
 if not 0<=start<end<=millis('2026-08-01'):raise ValueError('Use a completed pre-August 2026 research range; final-test data stays excluded')
 destination=pathlib.Path(destination)
 destination.mkdir(mode=0o700,parents=True,exist_ok=False)
 urls=[BASE+f'Kraken_OHLCVT_{quarter}.zip'] if quarter else [BASE+f'Kraken_OHLCVT_Full.zip.part{i:02d}' for i in range(5)]
 remote=RemoteParts(urls);files=[]
 with zipfile.ZipFile(remote) as archive:
  for market,aliases in PAIRS.items():
   options=[n for n in archive.namelist() if pathlib.PurePosixPath(n).name in [a+'_1.csv' for a in aliases]]
   if len(options)!=1:raise ValueError(f'{market}: need exactly one one-minute EUR file, found {options}')
   info=archive.getinfo(options[0]);digest=hashlib.sha256();source_digest=hashlib.sha256();count=0;previous=-1
   output=destination/(market+'_1.csv');temporary=output.with_suffix('.partial')
   with archive.open(info) as source,temporary.open('xb') as target:
    while True:
     row=source.readline(4097)
     if not row:break
     if len(row)>4096:raise ValueError('Oversized CSV row')
     source_digest.update(row)
     timestamp=int(row.split(b',',1)[0])*1000
     if timestamp<=previous:raise ValueError('Archive timestamps are not strictly increasing')
     previous=timestamp
     # Check time before handling prices; never retain final-test rows.
     if start<=timestamp<end:
      target.write(row);digest.update(row);count+=1
   if not count:raise ValueError('No requested observations for '+market)
   os.replace(temporary,output)
   files.append({'market':market,'file':output.name,'sha256':digest.hexdigest(),'rows':count,'archive_entry':info.filename,'archive_crc32':f'{info.CRC:08x}','archive_entry_sha256':source_digest.hexdigest()})
   print(f'{market}: retained {count:,} pre-holdout candles ({remote.downloaded:,} archive bytes fetched)',file=sys.stderr,flush=True)
 audit={'source':PAGE,'downloaded_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'start':start,'end':end,'archive_parts':remote.parts,'http_bytes_downloaded':remote.downloaded,'files':files,'verification':'Stable ETags, exact byte ranges, ZIP CRC and local CSV SHA-256. Full archive SHA-256 is not verified when fetching a subset. No zero filling or venue mixing.'}
 (destination/'origin.json').write_text(json.dumps(audit,indent=2)+'\n')
 return audit
if __name__=='__main__':
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('destination');parser.add_argument('--start',default='2022-11-01');parser.add_argument('--end',default='2026-08-01');parser.add_argument('--quarter',choices=['2026Q1','2026Q2'])
 args=parser.parse_args();result=download(args.destination,millis(args.start),millis(args.end),args.quarter)
 print(json.dumps(result,indent=2))
