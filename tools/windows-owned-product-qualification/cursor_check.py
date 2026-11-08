"""Bounded independent checker of private proof pixels, clocks and ownership receipts.
No Rust selector/math import, fixture actions never establish a candidate stationary prefix.
"""
from fractions import Fraction
import json
from pathlib import Path
import subprocess
import threading
import analyze

FORBIDDEN={'capture.json','capture.windows-v2','project.json','screen.mp4','session.json'}
def private_only(root):
    assert not any(p.name in FORBIDDEN for p in root.rglob('*')),'private proof leaked capture/import authority'

def pam(path):
    assert path.is_file() and not path.is_symlink() and path.stat().st_size<=9*1024*1024,'PAM cap/regular-file failure'
    data=path.read_bytes();header,body=data.split(b'ENDHDR\n',1);lines=header.splitlines()
    assert lines[0]==b'P7';fields=dict(x.split(b' ',1) for x in lines[1:])
    assert set(fields)=={b'WIDTH',b'HEIGHT',b'DEPTH',b'MAXVAL',b'TUPLTYPE'}
    w,h=int(fields[b'WIDTH']),int(fields[b'HEIGHT'])
    assert 0<w<=1920 and 0<h<=1080 and fields[b'DEPTH']==b'4' and fields[b'MAXVAL']==b'255' and fields[b'TUPLTYPE']==b'RGB_ALPHA'
    assert len(body)==w*h*4,'truncated/raw overflow PAM'
    return w,h,body

def anchors(w,h,pixels,raw_width=None,raw_height=None):
    rw,rh=(w if raw_width is None else raw_width),(h if raw_height is None else raw_height)
    assert 0<rw<=w and 0<rh<=h
    result=[]
    for value in (40,100,160):
        points={(i%w,i//w) for i in range(w*h) if pixels[4*i:4*i+4]==bytes([value,value,value,255])}
        components=[]
        while points:
            seed=points.pop();todo=[seed];region=[seed]
            while todo:
                x,y=todo.pop()
                for n in [(x-1,y),(x+1,y),(x,y-1),(x,y+1)]:
                    if n in points:points.remove(n);todo.append(n);region.append(n)
            if len(region)==256:
                left=min(x for x,y in region);top=min(y for x,y in region)
                if max(x for x,y in region)==left+15 and max(y for x,y in region)==top+15:components.append([left+8,top+8])
        assert len(components)==1,'missing/ambiguous native 16px fiducial'
        x,y=components[0];assert x+8<=rw and y+8<=rh,'fiducial extends into raw padding'
        result.append(components[0])
    return result

def unit_origin(raw,physical):
    assert len(raw)==len(physical)>=3
    a,b,c=raw[:3];assert (b[0]-a[0])*(c[1]-a[1])!=(b[1]-a[1])*(c[0]-a[0]),'collinear anchors'
    offsets={(p[0]-r[0],p[1]-r[1]) for r,p in zip(raw,physical)}
    assert len(offsets)==1,'shifted/scaled fiducials do not prove unit translation'
    return list(offsets.pop())

def bracket(row,clock):
    q0,q1=row['q0'],row['q1'];f,e,a,s,n=(clock[k] for k in ['frequency','epoch','admission','cutoff','written'])
    assert f>0 and 0<=a<=q0<=q1<s and 30*(q1-q0)<=f,'invalid/overwide QPC bracket'
    lo=Fraction(q0*10000000-e*f,f*10000000);hi=Fraction(q1*10000000-e*f,f*10000000)
    assert 0<=lo<=hi<Fraction(n,30),'cross-boundary QPC bracket'
    mid=q0+(q1-q0)//2;at=(mid*10000000-e*f)*1000000//(f*10000000)
    assert 0<=at<n*1000000//30
    return dict(atUs=at,uncertaintyQpc=q1-q0,physicalLatencyClaim=False)

def raster(directory,index,shape):
    w,h,source=pam(directory/shape['path']);assert (w,h)==(shape['width'],shape['height']) and 0<=shape['hotspot'][0]<w and 0<=shape['hotspot'][1]<h
    native_path=directory/f'native-color-{index}.pam'
    if native_path.exists() or any(0<a<255 for a in source[3::4]):
        nw,nh,native=pam(native_path);assert (nw,nh)==(w,h),'native color dimensions disagree'
        assert any(native[3::4]),'missing native color alpha'
        for i in range(w*h):
            p=native[i*4:i*4+4];a=p[3]
            assert all(v<=a for v in p[:3]),'native color is not premultiplied'
            expected=bytes([(v*255+a//2)//a for v in p[:3]]+[a]) if a else bytes(4)
            assert source[i*4:i*4+4]==expected,'candidate disagrees with retained native premultiplied color'
    for bg in range(4):
        rw,rh,got=pam(directory/f'raster-{index}-{bg}.pam');assert (rw,rh)==(w,h)
        for i in range(w*h):
            x,y=i%w,i//w;background=([0]*3 if bg==0 else [255]*3 if bg==1 else ([35]*3 if (x//4+y//4)%2==0 else [210]*3) if bg==2 else [37,91,173])
            p=source[i*4:i*4+4]
            assert p[3] or p[:3]==bytes(3),'zero alpha not canonical'
            for c in range(3):assert abs(got[i*4+c]-(p[c]*p[3]+background[c]*(255-p[3])+127)//255)<=1,'native raster mismatch'
    return source

def overlay(before,after,w,h,shape,pixels,hotspot):
    left,top=hotspot[0]-shape['hotspot'][0],hotspot[1]-shape['hotspot'][1]
    assert len(before)==len(after)==w*h*4
    changed=0
    for i in range(w*h):
        x,y=i%w,i//w;u,v=x-left,y-top;a=0;p=None
        if 0<=u<shape['width'] and 0<=v<shape['height']:p=pixels[(v*shape['width']+u)*4:][:4];a=p[3]
        for c in range(3):
            expected=(p[c]*a+before[i*4+c]*(255-a)+127)//255 if p else before[i*4+c]
            assert abs(after[i*4+c]-expected)<=1,'actual WGC cursor-free/embedded pixels disagree'
        changed+=before[i*4:i*4+3]!=after[i*4:i*4+3]
    assert changed>0,'WithCursor control has no actual cursor pixels'

def codec_crop(actual,expected,w,h,crop):
    assert 0<w<=1920 and 0<h<=1080 and w%2==h%2==0,'420 coded dimensions must be bounded and even'
    assert len(actual)==len(expected)==w*h*3//2,'420 native plane byte count mismatch'
    left,top,right,bottom=crop;assert 0<=left<right<=w and 0<=top<bottom<=h,'invalid measured codec crop'
    chroma=[left//2,top//2,(right+1)//2,(bottom+1)//2];metrics={}
    for name,stride,offset,bounds in [('Y',w,0,crop),('U',w//2,w*h,chroma),('V',w//2,w*h*5//4,chroma)]:
        x0,y0,x1,y1=bounds;errors=[abs(actual[offset+y*stride+x]-expected[offset+y*stride+x]) for y in range(y0,y1) for x in range(x0,x1)]
        metrics[name]=dict(max=max(errors),mean=sum(errors)/len(errors),samples=len(errors),bounds=list(bounds))
        assert max(errors)<=48 and sum(errors)<=12*len(errors),f'codec-tolerant 420 {name} crop mismatch: max={max(errors)}, mean={sum(errors)/len(errors)}'
    return metrics

def startup(trace,info):
    s=trace['startup'];assert isinstance(s,dict),'missing scalar startup receipt'
    for key in ['itemCreations','captureStarts','cleanupConsumptions','metadataCallbacks','mismatches','pixelCopies','preArmPixelCopies','preAdmissionPixelCopies','sourcePublications','preArmSourcePublications','proofWrites','preArmProofWrites']:
        assert type(s[key]) is int and 0<=s[key]<2**(32 if key in {'itemCreations','captureStarts','cleanupConsumptions'} else 64),'invalid startup counter: '+key
    stages=[s[k] for k in ['startQpc','freezeQpc','probeQpc','preparedQpc','gate2Qpc','encoderQpc','gate4Qpc','parkEnterQpc','parkLeaveQpc','supervisorQpc','writerQpc','armQpc']]
    assert all(type(t) is int and 0<=t<2**63 for t in stages) and stages==sorted(stages),'startup transition order disagrees'
    assert s['armQpc']==trace['admission'],'ARM/admission disagreement'
    assert s['itemCreations']==s['captureStarts']==s['cleanupConsumptions']==1,'startup must use one item, capture and cleanup'
    assert all(s[k]==0 for k in ['preArmPixelCopies','preAdmissionPixelCopies','preArmSourcePublications','preArmProofWrites']),'pre-arm/admission pixel or proof activity'
    assert s['proofWrites']==2 and s['pixelCopies']>=s['sourcePublications']>=1,'incomplete private pixel receipt'
    assert s['metadataCallbacks']>=s['pixelCopies']+s['mismatches']+1,'metadata/copy counts disagree'
    assert len(s['allocation'])==2 and all(type(v) is int and 2<=v<=16384 for v in s['allocation']),'invalid allocation dimensions'
    raw=[trace['width'],trace['height']]
    assert s['frozen']==raw and info.get('rawSize',raw)==raw,'frozen media/proof dimensions disagree'
    assert all(type(v) is int and 2<=v<=limit for v,limit in zip(raw,[1920,1080])),'private media dimension cap'
    encoded=[v+v%2 for v in raw];assert info['encodedSize']==encoded,'coded dimensions disagree with frozen media'
    sources=[e for e in trace['events'] if e['kind']=='source']
    assert len(sources)==s['sourcePublications'] and sources[0]['native']==trace['epoch'],'first source/epoch disagreement'
    for index,event in enumerate(sources):
        assert event['seq']==index and event['native']*trace['frequency']>=trace['admission']*10000000 and event['qpc']>=trace['admission'],'source predates ARM/admission'
        assert [event['contentWidth'],event['contentHeight']]==[event['textureWidth'],event['textureHeight']]==raw,'source/frozen dimensions disagree'
    return encoded

def movie(directory,info,ffmpeg,ffprobe,rawanchors,reference=None,crop=None):
    summary=json.loads((directory/'proof-summary.json').read_text())
    assert summary['privateProof'] is True and summary['neverImport'] is True and summary['workerExit']==0 and summary['encoderSignaled'] is True,'Private proof completion was not verified'
    clock=summary['clock'];trace=json.loads((directory/'trace.json').read_text());picks=analyze.expected(trace)
    assert all(clock[k]==trace[k] for k in ['frequency','admission','epoch','cutoff','selected','written'])
    encoded=startup(trace,info)
    for index in range(2):
        rw,rh,_=pam(directory/f'raw-{index}.pam');assert [rw,rh]==encoded,'raw exemplar/frozen dimensions disagree'
    movie=directory/('proof-with-cursor.mp4' if info['withCursor'] else 'proof-without-cursor.mp4');assert movie.stat().st_size<=64*1024*1024
    meta=json.loads(subprocess.check_output([str(ffprobe),'-v','error','-show_streams','-show_packets','-of','json',movie],timeout=20,text=True))
    assert len(meta['streams'])==1;stream=meta['streams'][0];w,h=encoded;assert (stream['width'],stream['height'])==(w,h) and stream['codec_name']=='h264' and stream['pix_fmt']=='yuv420p';analyze.packets(meta['packets'],stream['time_base'],len(picks))
    diagnostics=dict(ffmpegVersion=subprocess.check_output([str(ffmpeg),'-version'],timeout=5,text=True).splitlines()[0],comparison='fiducials for dynamic proof',frames=[])
    if reference is not None:
        assert 0<w<=1920 and 0<h<=1080 and w%2==h%2==0 and len(reference)==w*h*4,'invalid full coded RGBA reference'
        assert stream.get('color_space') in {None,'unknown'} and stream.get('color_range') in {None,'unknown','tv'},'unsupported tagged movie color conversion'
        left,top,right,bottom=crop;crop=[max(0,left),max(0,top),min(w,right),min(h,bottom)]
        bgra=bytearray(reference);bgra[0::4]=reference[2::4];bgra[2::4]=reference[0::4]
        # Match the existing untagged BGRA->420 encoder conversion on the full frame; no reference x264 encode.
        command=[str(ffmpeg),'-v','error','-nostdin','-f','rawvideo','-pixel_format','bgra','-video_size',f'{w}x{h}','-framerate','30','-i','pipe:0','-map','0:v:0','-an','-frames:v','1','-pix_fmt','yuv420p','-f','rawvideo','pipe:1']
        converted=subprocess.run(command,input=bgra,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=20)
        assert converted.returncode==0 and len(converted.stdout)==w*h*3//2,'full-frame reference format conversion failed'
        expected=converted.stdout;diagnostics.update(comparison='native yuv420p planes; full coded reference converted before aligned cell crop',converterCommand=command,crop=crop)
    proc=subprocess.Popen([str(ffmpeg),'-v','error','-i',movie,'-f','rawvideo','-pix_fmt','rgb24','pipe:1'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL);timer=threading.Timer(20,proc.kill);timer.start();planar=None;planar_timer=None
    try:
        if reference is not None:
            planar=subprocess.Popen([str(ffmpeg),'-v','error','-i',movie,'-f','rawvideo','-pix_fmt','yuv420p','pipe:1'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL);planar_timer=threading.Timer(20,planar.kill);planar_timer.start()
        for index in range(len(picks)):
            pixels=proc.stdout.read(w*h*3);assert len(pixels)==w*h*3
            for (x,y),color in zip(rawanchors,[40,100,160]):assert all(abs(pixels[(y*w+x)*3+c]-color)<=24 for c in range(3)),'decoded native fiducial lost'
            if reference is not None:
                left,top,right,bottom=crop;errors=[abs(pixels[(y*w+x)*3+c]-reference[(y*w+x)*4+c]) for y in range(top,bottom) for x in range(left,right) for c in range(3)];assert errors,'empty static cursor crop'
                planes=codec_crop(planar.stdout.read(w*h*3//2),expected,w,h,crop)
                diagnostics['frames'].append(dict(index=index,rawRgbMax=max(errors),rawRgbMean=sum(errors)/len(errors),legacyRgbPassed=max(errors)<=48 and sum(errors)<=12*len(errors),planes=planes))
        assert proc.stdout.read(1)==b'' and proc.wait(timeout=5)==0
        if planar is not None:assert planar.stdout.read(1)==b'' and planar.wait(timeout=5)==0
    finally:
        for child,deadline in [(proc,timer),(planar,planar_timer)]:
            if child is not None:
                deadline.cancel()
                if child.poll() is None:child.kill();child.wait(timeout=5)
    return clock,len(picks),diagnostics

EXPECTED_ACTIONS=['alpha','restore-cursor','alpha','hidden','restore-cursor','occlude','unocclude','move-away-back','move','move-back','minimize-restore','resize','close']

def resize_attribution(trace,summary,first_write,observation,actions):
    def integer(value,label,minimum=0,maximum=2**63):
        assert type(value) is int and minimum<=value<maximum,'invalid resize scalar: '+label
        return value
    def qpc(row,key):return integer(row[key],key)
    frequency=integer(trace['frequency'],'frequency',1)
    assert actions['overflow'] is False and integer(actions['qpcFrequency'],'source frequency',1)==frequency,'resize action clock/overflow disagreement'
    records=actions['records']
    assert [r['action'] for r in records]==EXPECTED_ACTIONS,'resize source action schedule changed'
    previous=0
    for index,row in enumerate(records):
        assert integer(row['sequence'],'action sequence',1)==index+1 and row['win32Succeeded'] is True,'resize source action failed/sequence changed'
        before,after=qpc(row,'operationBeforeQpc'),qpc(row,'operationAfterQpc')
        assert previous<=before<=after,'resize source operations reordered'
        previous=after
    action=records[11];close=records[12]
    assert integer(observation['actionSequence'],'observed action sequence',1)==action['sequence'],'resize observation belongs to another action'
    rectangles=[]
    for key in ['windowRectBefore','windowRectAfter']:
        r=action[key];assert isinstance(r,list) and len(r)==4,'missing actual resize rectangle'
        for value in r:integer(value,key,-2**31,2**31)
        assert r[0]<r[2] and r[1]<r[3],'invalid actual resize rectangle'
        rectangles.append(r)
    before,after=rectangles;delta=[(after[i+2]-after[i])-(before[i+2]-before[i]) for i in [0,1]]
    assert before[:2]==after[:2] and delta==[2,2],'resize must retain actual origin and grow exactly 2+2'
    assert observation['workerAliveBeforeResize'] is True and observation['workerEndedBeforeFinish'] is True and observation['manualStopRequested'] is False,'resize was not live and automatically ended before Finish'
    admission=qpc(trace,'admission');cutoff=qpc(trace,'cutoff');first=qpc(first_write,'qpc')
    live_before=qpc(observation,'liveCheckBeforeQpc');live_after=qpc(observation,'liveCheckAfterQpc');ended=qpc(observation,'endedObservedQpc')
    read_before=qpc(action,'commandReadBeforeQpc');read_after=qpc(action,'commandReadAfterQpc');operation_before=qpc(action,'operationBeforeQpc');operation_after=qpc(action,'operationAfterQpc')
    assert qpc(records[10],'operationAfterQpc')<=qpc(trace['startup'],'startQpc')<=admission,'restore overlaps resize worker startup'
    assert admission<=first<=live_before<=live_after<=read_before<=read_after<=operation_before,'resize lacks fresh admission/first write/exact-child liveness'
    assert operation_before<=operation_after<=ended,'resize end predates native operation completion'
    assert ended<qpc(close,'commandReadBeforeQpc')<=qpc(close,'commandReadAfterQpc')<=qpc(close,'operationBeforeQpc'),'source closed before resize worker ended'
    geometry=[e for e in trace['events'] if e['kind']=='geometry']
    assert len(geometry)==1,'resize requires exactly one geometry cutoff'
    event=geometry[0];event_qpc=qpc(event,'qpc')
    assert operation_before<event_qpc==cutoff<=ended,'geometry cutoff does not follow this live resize action'
    raw=[integer(trace[k],k,2,limit+1) for k,limit in [('width',1920),('height',1080)]]
    assert trace['startup']['frozen']==raw,'resize frozen grid disagrees'
    content=[integer(event[k],k,1,16385) for k in ['contentWidth','contentHeight']]
    texture=[integer(event[k],k,1,16385) for k in ['textureWidth','textureHeight']]
    assert content==[v+d for v,d in zip(raw,delta)],'rejected content is unrelated to measured resize delta'
    sources=[e for e in trace['events'] if e['kind']=='source']
    assert sources and trace['writes'],'resize has no admitted source/write'
    for source in sources:
        assert [source['contentWidth'],source['contentHeight']]==[source['textureWidth'],source['textureHeight']]==raw,'resize admitted changed geometry'
        assert admission<=qpc(source,'qpc')<cutoff,'resize source publication outside admission/cutoff'
    sequence=integer(first_write['seq'],'first write sequence')
    decisions=[e for e in trace['events'] if e['kind']=='decision']
    assert decisions and sequence==trace['writes'][0]['seq']==decisions[0]['seq'],'resize first write/selection sequence disagrees'
    selected=[source for source in sources if source['seq']==sequence]
    assert len(selected)==1 and qpc(selected[0],'qpc')<=first,'resize first write lacks causal publication'
    clock=summary['clock']
    assert all(clock[k]==trace[k] for k in ['frequency','admission','epoch','cutoff','selected','written']),'resize summary/trace clock disagreement'
    assert trace['sourceClosed'] is False and trace['sourceAvailabilityEnd'] is None and clock['geometry_changed'] is True and clock['closed'] is False and clock['source_availability'] is None,'resize has wrong automatic terminal cause'
    return dict(actionSequence=action['sequence'],frozenSize=raw,rejectedContentSize=content,rejectedTextureSize=texture,windowSizeDelta=delta,operationBeforeQpc=operation_before,operationAfterQpc=operation_after,cutoffQpc=cutoff,firstWriteQpc=first,liveCheckBeforeQpc=live_before,liveCheckAfterQpc=live_after,endedObservedQpc=ended,workerEndedBeforeFinish=True,manualStopRequested=False,oldTextureNewContentObserved=texture==raw and content!=raw)

def challenge_termination(trace,summary,rows):
    clock=summary['clock'];cutoff=trace['cutoff']
    assert type(cutoff) is int and cutoff>=0 and clock['cutoff']==cutoff,'challenge cutoff disagreement'
    assert trace['sourceClosed'] is False and trace['sourceAvailabilityEnd'] is None and clock['closed'] is False and clock['source_availability'] is None,'unexpected automatic challenge termination'
    events=[e for e in trace['events'] if e['kind']=='geometry']
    assert type(clock['geometry_changed']) is bool,'invalid challenge termination flag'
    if clock['geometry_changed']:assert len(events)==1 and events[0]['qpc']==cutoff,'challenge geometry/summary termination disagreement'
    else:assert not events,'challenge manual Finish has geometry event'
    before=[]
    for index,row in enumerate(rows):
        assert all(type(row[k]) is int and row[k]>=0 for k in ['q0','q1']) and row['q0']<=row['q1'],'invalid challenge observation clocks'
        if trace['admission']<=row['q0']<=row['q1']<cutoff:before.append(index)
    return dict(terminationCause='geometryChanged' if events else 'manualFinish',cutoffQpc=cutoff,observationsWhollyBeforeCutoff=before,observationsOutsideRecordingInterval=[i for i in range(len(rows)) if i not in before])

def check(case,ffmpeg,ffprobe):
    private_only(case);results={};raw={};controls={}
    for mode in ['without','with','alpha-without','alpha-with']:
        d=case/mode;info=json.loads((d/'geometry.json').read_text());w,h,pixels=pam(d/'raw-0.pam');rw,rh=info['rawSize'];assert [w,h]==info['encodedSize']==[rw+rw%2,rh+rh%2]
        native=anchors(w,h,pixels,rw,rh);origin=unit_origin(native,info['physicalAnchors']);g=info['geometry'];assert origin in [g['window'][:2],g['dwm'][:2],g['clientOrigin']],'no independently measured origin candidate matched'
        # Validate padding once; no crop, fitted scale or DPI multiplication.
        assert all(pixels[(y*w+x)*4:(y*w+x)*4+3]==bytes(3) for y in range(h) for x in range(w) if x>=rw or y>=rh),'right/bottom padding not black'
        assert info['withCursor'] is (mode in ['with','alpha-with']) and len(info['observations'])==3,'stationary pair mode/observation count mismatch'
        row0=info["observations"][0];s0=row0["shape"];assert s0 is not None;left=row0["position"][0]-origin[0]-s0["hotspot"][0];top=row0["position"][1]-origin[1]-s0["hotspot"][1];clock,frames,movie_diagnostics=movie(d,info,ffmpeg,ffprobe,native,pixels,[left-2,top-2,left+s0["width"]+2,top+s0["height"]+2]);times=[]
        assert info['telemetry']==[] and info['prefix']=='rejectedAll' and info['physicalLatencyClaim'] is False
        for i,row in enumerate(info['observations']):
            assert row['telemetryAccepted'] is False and row['before']==row['after']==g and row['consistent'] is True,'ambiguous stationary required profile'
            assert row['position']==row['cursorInfoPosition'] and row['flags']==1 and row['hitRoot']==info['selectedHwnd'] and row['hitPid']==g['pid'] and row['hitThread']==g['thread'],'cursor not owned/visible'
            assert row['shape'] is not None and row['imageError'] is None,'required static cursor extraction failed'
            pixels_shape=raster(d,i,row['shape']);times.append(bracket(row,clock))
            signature=(g,origin,[rw,rh],[w,h],info['physicalAnchors'],info['selectedHwnd'],row['position'],row['shape']['hotspot'],row['shape']['width'],row['shape']['height'],pixels_shape)
            if i==0:controls[mode]=signature
            assert signature==controls[mode],'stationary cursor changed between observations'
            if mode.startswith('alpha-'):assert any(0<a<255 for a in pixels_shape[3::4]),'alpha pair has no actual partial alpha'
        shape=row0['shape'];shape_pixels=pixels_shape;hotspot=[row0['position'][j]-origin[j] for j in [0,1]]
        exemplars=[]
        for index in range(2):
            ew,eh,exemplar=pam(d/f'raw-{index}.pam');assert (ew,eh)==(w,h) and anchors(w,h,exemplar,rw,rh)==native,'stationary raw geometry changed'
            exemplars.append(exemplar)
        raw[mode]=exemplars;results[mode]=dict(frames=frames,movieDiagnostics=movie_diagnostics,origin=origin,rawAnchors=native,times=times,normalization='physical hotspot minus proven unit origin divided by encoded dimensions once; no telemetry admitted')
        if mode in ['with','alpha-with']:
            without='alpha-without' if mode=='alpha-with' else 'without'
            assert controls[without]==controls[mode],'cursor pair geometry, placement or shape changed'
            for before,after in zip(raw[without],exemplars):overlay(before,after,w,h,shape,shape_pixels,hotspot)
    assert controls['with'][:7]==controls['alpha-with'][:7],'alpha pair moved the source or physical hotspot'
    challenge=case/'challenge';control=json.loads((challenge/'challenges.json').read_text());assert control['telemetry']==[] and control['prefix']=='rejectedAll' and control['noResumeAfterEqualPolls'] is True
    rows=control['observations'];actions=json.loads((case/'source-actions.json').read_text());assert actions['overflow'] is False
    expected_actions=EXPECTED_ACTIONS
    assert [r['action'] for r in actions['records']]==expected_actions and all(r['win32Succeeded'] for r in actions['records'])
    assert len(rows)==9 and all(r['telemetryAccepted'] is False and r['q0']<=r['q1'] for r in rows)
    assert rows[0]['shape'] is not None and rows[0]['imageError'] is None;alpha=raster(challenge,0,rows[0]['shape']);assert any(0<a<255 for a in alpha[3::4]),'partial alpha fixture never observed'
    assert rows[1]['flags']==0,'hidden cursor was not observed'
    assert rows[3]['hitRoot']!=info['selectedHwnd'] and rows[3]['hitPid']==info['geometry']['pid'],'owned occluder was not observed'
    assert rows[5]['before']==rows[5]['after'] and rows[5]['telemetryAccepted'] is False,'move-away/back equal polls resumed cursor'
    assert rows[6]['before']['window']!=rows[7]['before']['window'],'same-size move not observed'
    assert rows[7]['before']['window']==info['geometry']['window'],'move-back did not restore fixture geometry'
    trace=json.loads((challenge/'trace.json').read_text());termination=challenge_termination(trace,json.loads((challenge/'proof-summary.json').read_text()),rows)
    challenge_info=json.loads((case/'with/geometry.json').read_text());challenge_info['observations']=rows
    _,challenge_frames,challenge_diagnostics=movie(challenge,challenge_info,ffmpeg,ffprobe,results['with']['rawAnchors'])
    visibility=json.loads((case/'visibility/visibility.json').read_text());assert visibility['telemetry']==[] and visibility['prefix']=='rejectedAll';v=visibility['observations'];assert len(v)==3 and all(row['telemetryAccepted'] is False for row in v);assert v[2]['hitRoot']!=info['selectedHwnd'],'outside point still belongs to target'
    resize=case/'resize';resize_trace=json.loads((resize/'trace.json').read_text())
    attribution=resize_attribution(resize_trace,json.loads((resize/'proof-summary.json').read_text()),json.loads((resize/'first-write-started.json').read_text()),json.loads((resize/'resize-observation.json').read_text()),actions)
    resize_raw=[resize_trace['width'],resize_trace['height']];resize_info=dict(withCursor=True,rawSize=resize_raw,encodedSize=[v+v%2 for v in resize_raw])
    rw,rh,resize_pixels=pam(resize/'raw-0.pam');assert [rw,rh]==resize_info['encodedSize'],'resize raw proof dimensions disagree'
    _,resize_frames,resize_diagnostics=movie(resize,resize_info,ffmpeg,ffprobe,anchors(rw,rh,resize_pixels,*resize_raw))
    results['explicitResize']=dict(attribution,frames=resize_frames,movieDiagnostics=resize_diagnostics)
    closed=case/'closed';close=json.loads((closed/'close-observation.json').read_text());assert close['workerEndedBeforeFinish'] and close['manualStopRequested'] is False
    trace_close=json.loads((closed/'trace.json').read_text());assert trace_close['sourceClosed'] is True or trace_close['sourceAvailabilityEnd']=='unavailable','close lacks automatic cause'
    close_info=dict(withCursor=True,encodedSize=[trace_close['width']+trace_close['width']%2,trace_close['height']+trace_close['height']%2])
    cw,ch,close_pixels=pam(closed/'raw-0.pam')
    assert [cw,ch]==close_info['encodedSize'],'resized raw proof dimensions disagree'
    _,closed_frames,closed_diagnostics=movie(closed,close_info,ffmpeg,ffprobe,anchors(cw,ch,close_pixels))
    results['automaticClose']=dict(frames=closed_frames,movieDiagnostics=closed_diagnostics,cause='wgcClosed' if trace_close['sourceClosed'] else 'sourceUnavailableStop')
    resource=json.loads((case/'resource-paths.json').read_text());assert resource['iterations']==16 and resource['before']==resource['after'] and resource['partialAcquisitionsOwned'] is True and resource['afterDibitsFailure'] is True and resource['afterColorRasterFailure'] is True and resource['afterColorDrawFailure'] is True and resource['nativeXorRejected'] is True
    topology=json.loads((case/'native-topology.json').read_text());assert topology['actualMonitors'] and topology['profileAwareness']=='PMv2 only'
    results['challenges']=dict(frames=challenge_frames,movieDiagnostics=challenge_diagnostics,actions=expected_actions,telemetryAccepted=0,**termination)
    result=json.loads((case/'proof-result.json').read_text());assert result['gdiStart']==result['gdiEnd'] and result['positionRestored'] and result['dpiRestored'] and result['showCountMutated'] is False
    results['scope']='private raw/clock/static cursor controls only';results['telemetryAccepted']=0;results['productionCursorQualified']=False
    (case/'cursor-analysis.json').write_text(json.dumps(results,indent=2)+'\n');return results
