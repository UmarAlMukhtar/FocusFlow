"""Independent integer contract and lossy visual checks; never imports the Rust selector."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import threading
from fractions import Fraction

TRACE_LIMIT=4*1024*1024
MEDIA_LIMIT=64*1024*1024

def expected(t):
    assert not t['overflow'], 'trace overflow'
    assert t.get('failure') is None and not t.get('supervisorTerminated'), 'failed worker trace'
    assert len(t['events'])+len(t['writes'])<=8192
    f,a,e,s=(t[k] for k in ['frequency','admission','epoch','cutoff'])
    assert f>0 and min(a,e,s)>=0
    n=max(0,((s*10000000-e*f)*30)//(f*10000000))
    assert n>0 and n==t['selected']==t['written']==len(t['writes'])
    assert len(t['events'])>0
    published=[]; picks=[]; last=None
    for order,event in enumerate(t['events']):
        assert event['order']==order,'noncontiguous frontier order'
        if event['kind']=='source':
            assert (event['contentWidth'],event['contentHeight'],event['textureWidth'],event['textureHeight'])==(t['width'],t['height'],t['width'],t['height']), 'mixed source geometry'
            if last:assert event['seq']>last['seq'] and event['native']>=last['native'] and event['qpc']>=last['qpc']
            assert event['native']>=0 and event['qpc']>=0
            last=event
            if event['native']*f>=a*10000000:published.append(event)
        elif event['kind']=='decision':
            i=len(picks); assert event['ordinal']==i and i<n
            assert published and published[0]['native']==e,'wrong first admitted epoch'
            assert ((min(s,event['qpc'])*10000000-e*f)*30)//(f*10000000)>i,'incomplete cell'
            candidates=[x for x in published if x['qpc']<=event['qpc'] and 30*(x['native']-e)<=i*10000000]
            assert candidates,'missing causal source'
            choice=max(candidates,key=lambda x:(x['native'],x['seq']))
            assert choice['seq']==event['seq'],'wrong causal selection'
            picks.append(choice)
        elif event['kind']=='geometry':
            assert (event['contentWidth'],event['contentHeight'])!=(t['width'],t['height']) or (event['textureWidth'],event['textureHeight'])!=(t['width'],t['height'])
        else:raise AssertionError('unknown trace event')
    assert len(picks)==n
    size=(t['width']+t['width']%2)*(t['height']+t['height']%2)*4
    for i,(write,pick) in enumerate(zip(t['writes'],picks)):
        assert write['ok'] and write['ordinal']==i and write['seq']==pick['seq'] and write['bytes']==size,'partial/wrong write'
    return picks

def identity(rgb,ew,eh,w,h):
    assert ew==w+w%2 and eh==h+h%2 and len(rgb)==ew*eh*3
    assert w>=160 and h>=120
    def values(x,y):
        return [rgb[(yy*ew+xx)*3+c] for yy in range(y-1,y+2) for xx in range(x-1,x+2) for c in range(3)]
    def bits(y):
        v=0
        for b in range(16):
            cell=values(20+b*8,y+4)
            assert max(cell)<70 or min(cell)>180,'ambiguous or obscured marker'
            if min(cell)>180:v|=1<<b
        return v
    paint,nonce=bits(40),bits(56)
    assert paint==bits(72) and nonce==bits(88),'torn marker replicas'
    corners=[]
    for (x,y),reference in zip([(8,8),(w-8,8),(8,h-8),(w-8,h-8)],[40,100,160,220]):
        cell=values(x,y);assert all(abs(c-reference)<=24 for c in cell),'wrong/obscured content corner'
        corners.append(round(sum(cell)/len(cell)))
    return paint,nonce,corners

def check_identity(rgb,ew,eh,source,w,h):
    paint,nonce,corners=identity(rgb,ew,eh,w,h)
    assert paint==source['paint'] and nonce==source['nonce'],'wrong source identity/frame order'
    assert all(abs(a-b)<=24 for a,b in zip(corners,source['corners']))
    return dict(paint=paint,nonce=nonce,corners=corners)

def packets(rows,timebase,n):
    assert len(rows)==n
    tb=Fraction(timebase)
    for i,p in enumerate(rows):
        assert int(p['pts'])==int(p['dts'])
        assert int(p['pts'])*tb==Fraction(i,30)
        assert int(p['duration'])*tb==Fraction(1,30)
    assert (int(rows[-1]['pts'])+int(rows[-1]['duration']))*tb==Fraction(n,30)

def analyze(case,ffmpeg,ffprobe):
    video=case/'screen.mp4'; assert video.stat().st_size<=MEDIA_LIMIT
    tracepath=case/'trace.json'; assert tracepath.stat().st_size<=TRACE_LIMIT
    trace=json.loads(tracepath.read_text(encoding='utf-8')); picks=expected(trace)
    meta=json.loads(subprocess.check_output([str(ffprobe),'-v','error','-show_streams','-show_packets','-of','json',str(video)],timeout=20,encoding='utf-8'))
    streams=meta['streams']; assert len(streams)==1
    stream=streams[0]; assert stream['codec_name']=='h264' and stream['pix_fmt']=='yuv420p'
    w,h=trace['width'],trace['height'];ew,eh=stream['width'],stream['height']
    assert (ew,eh)==(w+w%2,h+h%2),'wrong decoded geometry'
    packets(meta['packets'],stream['time_base'],len(picks))
    process=subprocess.Popen([str(ffmpeg),'-v','error','-i',str(video),'-map','0:v:0','-f','rawvideo','-pix_fmt','rgb24','pipe:1'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    watchdog=threading.Timer(20,process.kill);watchdog.start()
    decoded=[]
    try:
        for source in picks:
            data=process.stdout.read(ew*eh*3)
            decoded.append(check_identity(data,ew,eh,source,w,h))
        assert process.stdout.read(1)==b'', 'extra decoded frame'
        assert process.wait(timeout=10)==0,process.stderr.read(4096)
    finally:
        watchdog.cancel()
        if process.poll() is None:process.kill();process.wait(timeout=5)
    equal=sum(b['native']==a['native'] for a,b in zip([x for x in trace['events'] if x['kind']=='source'],[x for x in trace['events'] if x['kind']=='source'][1:]))
    result=dict(frames=len(decoded),decoded=decoded,packets=meta['packets'],stream=stream,equalNativePairs=equal,nativeEqualityCovered=equal>0,oldTextureNewContentObserved=any(x["kind"]=="geometry" and (x["textureWidth"],x["textureHeight"])==(w,h) and (x["contentWidth"],x["contentHeight"])!=(w,h) for x in trace["events"]),videoSha256=hashlib.sha256(video.read_bytes()).hexdigest(),limitations=['Identical callback visuals cannot independently identify callback sequence.','Instrumentation changes scheduling.','Finite geometry/device matrix.'])
    (case/'analysis.json').write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8')
    return result

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('case',type=Path);p.add_argument('--ffmpeg',type=Path,required=True);p.add_argument('--ffprobe',type=Path,required=True);a=p.parse_args()
    print(json.dumps({k:v for k,v in analyze(a.case,a.ffmpeg,a.ffprobe).items() if k not in ['decoded','packets','stream']}))
