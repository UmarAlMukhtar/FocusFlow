"""Independent production-source checks. A passing source receipt never enables a family."""
from fractions import Fraction
import hashlib
import json
from pathlib import Path
import re
import os
import subprocess
import time
from contextlib import closing
import cursor_check


def media_output(argv, limit, receipts, input_bytes=None, timeout=20):
    """Drain only the owned media child; nonblocking pipes keep deadlines enforceable."""
    command=list(map(str,argv));started=time.monotonic();stderr=bytearray();stderr_bytes=total=sent=0
    child=subprocess.Popen(command,stdin=subprocess.PIPE if input_bytes is not None else subprocess.DEVNULL,
                           stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    killed=False;out_open=err_open=True
    try:
        for stream in [child.stdout,child.stderr,child.stdin]:
            if stream is not None:os.set_blocking(stream.fileno(),False)
        while out_open or err_open or child.poll() is None:
            if time.monotonic()-started>=timeout:raise TimeoutError('owned media child execution deadline')
            progressed=False
            if child.stdin is not None and not child.stdin.closed:
                if sent==len(input_bytes):child.stdin.close()
                else:
                    try:sent+=os.write(child.stdin.fileno(),memoryview(input_bytes)[sent:sent+65536]);progressed=True
                    except BlockingIOError:pass
            if err_open:
                try:
                    data=os.read(child.stderr.fileno(),65536);progressed=bool(data)
                    if not data:err_open=False
                    stderr_bytes+=len(data);stderr.extend(data[:max(0,65536-len(stderr))])
                except BlockingIOError:pass
            if out_open:
                try:
                    data=os.read(child.stdout.fileno(),min(65536,limit-total+1))
                    if not data:out_open=False
                    else:
                        progressed=True;total+=len(data)
                        assert total<=limit,'owned media stdout cap'
                        yield data
                except BlockingIOError:pass
            if not progressed:time.sleep(.002)
        assert child.wait(timeout=5)==0,'owned media child failed: '+stderr.decode('utf-8','replace')
        assert input_bytes is None or sent==len(input_bytes),'incomplete media conversion input'
    finally:
        cleanup_started=time.monotonic()
        if child.poll() is None:child.kill();killed=True
        try:child.wait(timeout=5)
        finally:
            for stream in [child.stdout,child.stderr,child.stdin]:
                if stream is not None:stream.close()
            receipts.append(dict(argv=command,pid=child.pid,exitCode=child.returncode,killed=killed,
                reaped=child.returncode is not None,seconds=time.monotonic()-started,cleanupSeconds=time.monotonic()-cleanup_started,
                executionDeadlineSeconds=timeout,reapDeadlineSeconds=5,stdoutBytes=total,stderrBytes=stderr_bytes,
                stderr=stderr.decode('utf-8','replace')))


def media_frames(argv, frame_bytes, count, receipts, input_bytes=None):
    assert frame_bytes>0 and count>0
    pending=bytearray();decoded=0
    with closing(media_output(argv,frame_bytes*count,receipts,input_bytes)) as chunks:
        for chunk in chunks:
            pending.extend(chunk)
            while len(pending)>=frame_bytes:
                frame=bytes(pending[:frame_bytes]);del pending[:frame_bytes];decoded+=1
                yield frame
        assert decoded==count and not pending,'short decoded frame stream'


def eligible(row, clock):
    def integer(value):
        assert type(value) is str and re.fullmatch(r"0|[1-9][0-9]*",value)
        n=int(value);assert n<2**63;return n
    f,e,a,z=(integer(clock[k]) for k in ('qpcFrequency','epoch100ns','admissionQpc','cutoffQpc'))
    q0,q1=integer(row['q0']),integer(row['q1']);n=clock['completedFrames']
    assert f>0 and type(n) is int and n>0
    lo,hi=Fraction(q0,f)-Fraction(e,10000000),Fraction(q1,f)-Fraction(e,10000000)
    return a<=q0<=q1<z and 30*(q1-q0)<=f and 0<=lo<=hi<Fraction(n,30)


def model_origin(raw, physical, observed_dwm):
    origin = cursor_check.unit_origin(raw, physical)
    assert origin == observed_dwm[:2], 'Independent anchors reject the fixed DWM model; never fit an offset'
    return origin


def unique(pairs):
    result={}
    for key,value in pairs:
        assert key not in result, 'duplicate JSON field'
        result[key]=value
    return result


def load(path,cap=65536):
    assert path.is_file() and not path.is_symlink() and path.stat().st_size<=cap
    with path.open('rb') as stream:data=stream.read(cap+1)
    assert len(data)<=cap
    return json.loads(data,object_pairs_hook=unique)


def rows(path,cap):
    assert path.is_file() and not path.is_symlink()
    with path.open('rb') as stream:
        while True:
            line=stream.readline(cap+1)
            if not line:return
            assert len(line)<=cap, 'observation record cap'
            yield json.loads(line,object_pairs_hook=unique)


def verify_interactions(poses,inputs,index,evidence):
    clock=index['clock'];frequency=int(clock['qpcFrequency']);epoch=int(clock['epoch100ns'])
    def button(row,logical):
        key=('right' if logical=='left' else 'left') if row['buttonsSwapped'] else logical
        assert type(row[key]) is bool
        return row[key]
    def point(number):
        row=inputs[number];assert row['type']=='observed' and eligible(row,clock)
        basis=poses[row['basisPoseRow']];assert basis['type']=='observed' and eligible(basis,clock)
        assert int(basis['q1'])<=int(row['q0']) and 30*(int(row['q1'])-int(basis['q1']))<=frequency
        assert basis['before']==basis['after'] and row['barrierRevisionBefore']==row['barrierRevisionAfter']==basis['barrierRevision']
        window=basis['before'];assert row['rootHwnd']==window['hwnd'] and row['hitPid']==window['pid'] and row['hitTid']==window['tid']
        value=Fraction(int(row['q1']),frequency)-Fraction(epoch,10000000);us=-(-value.numerator*1000000//value.denominator)
        return dict(x=row['physicalPosition'][0]-window['dwmRect'][0],y=row['physicalPosition'][1]-window['dwmRect'][1],timestamp=us/1000000)
    def edge(edge):
        old,new=inputs[edge['previousInputRow']],inputs[edge['observedInputRow']]
        logical=edge['button'];assert logical in ('left','right')
        assert not button(old,logical) and button(new,logical) and old['buttonsSwapped']==new['buttonsSwapped']
        assert edge['occurrenceQpc']==[int(old['q0']),int(new['q1'])] and edge['basisPoseRow']==new['basisPoseRow']
        point(edge['previousInputRow']);point(edge['observedInputRow'])
    assert not evidence['truncated']
    for click in evidence['clickEvidence']:edge(click)
    assert len(evidence['dragEvidence'])==len(evidence['compatibility']['drags'])
    for drag,compat in zip(evidence['dragEvidence'],evidence['compatibility']['drags']):
        edge(drag['startEdge']);numbers=drag['inputPointRows'];assert len(numbers)>=2 and numbers==sorted(set(numbers))
        measured=[point(i) for i in numbers];assert len(measured)==len(compat['points'])
        for got,want in zip(compat['points'],measured):
            assert got['x']==want['x'] and got['y']==want['y'] and abs(got['timestamp']-want['timestamp'])<1e-9
        if drag['endKind']=='interrupted':assert drag['releaseBracket'] is None
        else:
            assert drag['endKind']=='observedRelease' and drag['releaseBracket'] is not None
            assert not button(inputs[numbers[-1]],drag['button'])
    return dict(clicks=len(evidence['clickEvidence']),drags=len(evidence['dragEvidence']),buttons=sorted({d['button'] for d in evidence['dragEvidence']}),endKinds=sorted({d['endKind'] for d in evidence['dragEvidence']}))


def safe_file(root,path,cap):
    import run
    run.production_file(path,root)
    assert path.is_file() and path.stat().st_size<=cap,'evidence file type/size cap'
    return path


def decimal(value):
    assert type(value) is str and re.fullmatch(r'0|[1-9][0-9]*',value),'noncanonical clock integer'
    result=int(value);assert result<2**63
    return result


def raw_static(session,raw,encoded,geometry,dwm):
    assert len(raw)==2 and all(type(x) is int and 2<=x<=limit for x,limit in zip(raw,[1920,1080]))
    assert encoded==[v+v%2 for v in raw]
    reference=None
    for ordinal in range(2):
        path=safe_file(session,session/f'raw-{ordinal}.pam',9*1024*1024)
        w,h,pixels=cursor_check.pam(path);assert [w,h]==encoded
        anchors=cursor_check.anchors(w,h,pixels,*raw)
        origin=model_origin(anchors,geometry['physicalAnchors'],dwm)
        client=[anchors[0][0]-8,anchors[0][1]-8];cw,ch=geometry['clientSize']
        assert anchors==[[client[0]+8,client[1]+8],[client[0]+cw-8,client[1]+8],[client[0]+8,client[1]+ch-8]],'client anchor geometry mismatch'
        assert 0<=client[0] and 0<=client[1] and client[0]+cw<=raw[0] and client[1]+ch<=raw[1]
        x,y=[p-o for p,o in zip(geometry['initialPointerPosition'],origin)]
        crop=[x-3,y-5,x+13,y+11]
        assert 0<=crop[0]<crop[2]<=raw[0] and 0<=crop[1]<crop[3]<=raw[1]
        assert all(pixels[(v*w+u)*4:(v*w+u)*4+4]==bytes([96,96,96,255]) for v in range(crop[1],crop[3]) for u in range(crop[0],crop[2])),'raw source contains cursor pixels in the controlled patch'
        current=dict(origin=origin,clientOffset=client,anchors=anchors,crop=crop)
        if reference is None:reference=dict(current,pixels=pixels)
        else:assert current=={k:v for k,v in reference.items() if k!='pixels'},'raw exemplars disagree on geometry'
    return reference


def decoded_static(rgb,w,h,geometry,reference,nonce):
    assert len(rgb)==w*h*3,'short decoded RGB frame'
    def cell(x,y):
        assert 1<=x<w-1 and 1<=y<h-1
        return [rgb[(v*w+u)*3+c] for v in range(y-1,y+2) for u in range(x-1,x+2) for c in range(3)]
    for (x,y),value in zip(reference['anchors'],[40,100,160]):
        assert all(abs(v-value)<=24 for v in cell(x,y)),'decoded fiducial mismatch'
    ox,oy=reference['clientOffset']
    for y,value in [(40,1),(56,nonce),(72,1),(88,nonce)]:
        decoded=0
        for bit in range(16):
            values=cell(ox+20+bit*8,oy+y+4)
            assert max(values)<70 or min(values)>180,'ambiguous static marker'
            if min(values)>180:decoded|=1<<bit
        assert decoded==value,'decoded static paint/nonce mismatch'


def trace_clock(trace,clock):
    for source,target in [('frequency','qpcFrequency'),('admission','admissionQpc'),('epoch','epoch100ns'),('cutoff','cutoffQpc')]:
        assert type(trace[source]) is int and trace[source]==decimal(clock[target]),'trace/canonical clock mismatch'
    n=clock['completedFrames'];assert type(n) is int and 2<=n<=600
    assert clock['workerExitCode']==clock['encoderExitCode']==0
    assert trace['selected']==trace['written']==n,'trace/canonical count mismatch'
    return cursor_check.analyze.expected(trace)


def saved_grid(meta,encoded,n):
    assert len(meta['streams'])==1,'foreign media stream/audio'
    stream=meta['streams'][0]
    assert stream['index']==0 and stream['codec_type']=='video' and stream['codec_name']=='h264' and stream['pix_fmt']=='yuv420p'
    assert [stream['width'],stream['height']]==encoded
    assert stream.get('color_space') in {None,'unknown'} and stream.get('color_range') in {None,'unknown','tv'},'unsupported tagged color conversion'
    assert all(p['stream_index']==0 for p in meta['packets']),'foreign packet stream'
    cursor_check.analyze.packets(meta['packets'],stream['time_base'],n)
    assert int(stream['start_pts'])==0 and int(stream['duration_ts'])*Fraction(stream['time_base'])==Fraction(n,30),'saved stream endpoint mismatch'
    return stream


def check_static_media(root,ffmpeg,ffprobe,trace,index,geometry,raw_reference):
    session=root/'session-0';video=safe_file(root,session/'screen.mp4',64*1024*1024)
    n=index['clock']['completedFrames'];w,h=[v+v%2 for v in [trace['width'],trace['height']]]
    receipts=[];result=dict(children=receipts,productionCursorQualified=False)
    try:
        import run
        result['tools']={str(tool):dict(sha256=run.sha(tool),version=b''.join(media_output([tool,'-version'],65536,receipts)).decode('utf-8','replace').splitlines()[0]) for tool in [ffmpeg,ffprobe]}
        meta=json.loads(b''.join(media_output([ffprobe,'-v','error','-show_streams','-show_packets','-of','json',video],4*1024*1024,receipts)),object_pairs_hook=unique)
        stream=saved_grid(meta,[w,h],n)
        rgba=raw_reference['pixels'];bgra=bytearray(rgba);bgra[0::4]=rgba[2::4];bgra[2::4]=rgba[0::4]
        command=[ffmpeg,'-v','error','-nostdin','-f','rawvideo','-pixel_format','bgra','-video_size',f'{w}x{h}','-framerate','30','-i','pipe:0','-map','0:v:0','-an','-frames:v','1','-pix_fmt','yuv420p','-f','rawvideo','pipe:1']
        reference=b''.join(media_output(command,w*h*3//2,receipts,input_bytes=bgra));assert len(reference)==w*h*3//2
        import run
        nonce=run.production_tuple(root)[2]
        decode=[ffmpeg,'-v','error','-nostdin','-i',video,'-map','0:v:0','-f','rawvideo','-pix_fmt']
        with closing(media_frames(decode+['rgb24','pipe:1'],w*h*3,n,receipts)) as frames:
            for frame in frames:decoded_static(frame,w,h,geometry,raw_reference,nonce)
        maxima={name:dict(max=0,mean=0) for name in ['Y','U','V']}
        with closing(media_frames(decode+['yuv420p','pipe:1'],w*h*3//2,n,receipts)) as frames:
            for frame in frames:
                for plane,values in cursor_check.codec_crop(frame,reference,w,h,raw_reference['crop']).items():
                    maxima[plane]['max']=max(maxima[plane]['max'],values['max'])
                    maxima[plane]['mean']=max(maxima[plane]['mean'],values['mean'])
        result.update(decodedStaticContentChecked=True,decodedCursorFreePatchChecked=True,packetClockChecked=True,decodedFrames=n,codecPlaneWorst=maxima,stream=stream,packets=meta['packets'],callbackPixelIdentity='unmeasured: equal static pixels cannot identify selected callbacks')
        return result
    except BaseException as error:
        result['error']=repr(error);raise
    finally:
        (root/'production-media-check.json').write_text(json.dumps(result,indent=2)+'\n')


def check(root,ffmpeg,ffprobe):
    import run
    import package_evidence
    root=Path(root);session=root/'session-0';cw,ch,nonce,family=run.production_tuple(root)
    package_evidence.production_files(root)
    profile='pmv2_96_single_monitor_'+family
    def read(name,cap=4096):return load(safe_file(root,root/name,cap),cap)
    manifest=read('session-0/capture.json',4*1024*1024);index=read('session-0/cursor/index.json',65536)
    state=manifest['state'];clock=index['clock'];terminal=index['terminal'];n=clock['completedFrames']
    assert manifest['schemaVersion']==3 and manifest['captureId']=='session-0' and state['status']=='finalized'
    assert index['schemaVersion']==2 and index['model']=='dwmExtendedFrameTranslationV1' and index['profile']==profile
    assert index['spatialErrorPx'] is None and index['frameAlignmentErrorUs'] is None and index['animationPhase']=='unknown'
    assert index['observationsPath']=='cursor/observations.ndjson' and index['inputObservationsPath']=='cursor/input-observations.ndjson'
    timing=state['videoTiming'];assert clock==timing['clock']
    endpoint=timing['endpoint'];tb=endpoint['timeBase']
    assert endpoint['interval']=='halfOpen' and endpoint['provenance']=='savedPacketFullGrid'
    assert endpoint['endPts']*Fraction(tb['numerator'],tb['denominator'])==Fraction(n,30),'manifest endpoint mismatch'
    assert index['durationUs']==n*1000000//30
    assert safe_file(root,session/'capture.windows-v3',64).read_bytes()==b'focusflow-windows-capture-v3\n' and not (session/'capture.windows-v2').exists()
    receipt=read('production-observer-completion.json');source_receipt=read('production-source-receipt.json')
    assert receipt['profile']==source_receipt['profile']==profile and receipt['terminal']==source_receipt['terminal']==terminal
    assert receipt['poseJoined'] is receipt['inputJoined'] is receipt['filesUsable'] is True and receipt['degraded'] is False
    assert terminal['sourceCutoffQpc']==decimal(clock['cutoffQpc']) and terminal['stopKind']=='stop'
    assert source_receipt['clock']==clock and source_receipt['ordinaryImport'] is True and source_receipt['productionCursorQualified'] is False
    assert type(source_receipt['finishMs']) is int and 0<=source_receipt['finishMs']<=12000
    source_cleanup=run.require_production_cleanup(root,root.parent/(root.name+'-command.json'),complete=True)
    cleanup=read('session-0/cleanup.json');assert cleanup['workerSignaled'] is cleanup['encoderSignaled'] is cleanup['exactNoninheritableHandles'] is True
    selected=read('source/source-selection.json');hwnd,pid=selected['hwnd'],selected['sourcePid']
    assert source_cleanup['pid']==pid,'selected source and normal cleanup identity disagree'
    owner=read('session-0/owned.json');assert selected['driverPid']==owner['parent']
    assert read('source/source-ready.json')['pid']==pid
    for name in ['source/source-ready.json','source/source-observed.json']:
        value=read(name);assert value['hwnd']==hwnd and value['pid']==pid
    request=read('source/source-request.json');assert request==dict(width=cw,height=ch,nonce=nonce,animated=False,privateProfile='borderless' if family=='popup' else 'decorated')
    for name in ['production-source-ready.json','production-start-authorized.json','production-armed.json','production-actions-complete.json','production-phase.json','production-placement.json']:
        value=read(name);assert value['hwnd']==hwnd and value['sourcePid']==pid,'source owner changed'
    assert read('production-start-authorized.json')['action']=='static-owned-canvas-v1'
    assert read('production-actions-complete.json')['action']=='static-hold-complete-v1'
    armed=read('production-armed.json');assert armed['candidateOnly'] is True and armed['profile']==profile
    phase=read('production-phase.json');assert phase['phase']=='postcapture' and phase['workerEncoderJoined'] is phase['sourceClosed'] is True
    placement=read('production-placement.json');geometry=read('production-geometry.json')
    assert geometry['profile']==profile and geometry['clientSize']==placement['clientSize']==[cw,ch]
    point=geometry['initialPointerPosition'];assert placement['position']==point and placement['buttonsUp'] is True and placement['cursorFlags']==1 and placement['cursorHandle']>0
    assert placement['footprint']==[point[0]-3,point[1]-5,16,16] and placement['margin']==2
    assert point==[placement['geometry']['clientOrigin'][0]+cw//2,placement['geometry']['clientOrigin'][1]+ch//2]
    operations=placement['operations'];assert [o['operation'] for o in operations]==['place-center','wake-horizontal','wake-horizontal','settle-center']
    assert [o['dx'] for o in operations[1:3]]==[1,-1] and placement['wakeMs']==100 and placement['settleMs']==150
    last=0
    for op in operations:
        assert type(op['q0']) is int and type(op['q1']) is int and last<=op['q0']<=op['q1']<decimal(clock['admissionQpc']);last=op['q1']
    lanes={};total_bytes=total_records=0;cadence={}
    for filename,key,cap in [('observations.ndjson','pose',2048),('input-observations.ndjson','input',1024)]:
        path=safe_file(root,session/'cursor'/filename,128*1024*1024);digest=hashlib.sha256();values=[];size=0;last=None
        with path.open('rb') as stream:
            for line in iter(lambda:stream.readline(cap+1),b''):
                assert len(line)<=cap;size+=len(line);digest.update(line)
                assert total_bytes+size<=128*1024*1024 and total_records+len(values)<8192
                row=json.loads(line,object_pairs_hook=unique);q=[decimal(row['q0']),decimal(row['q1'])]
                assert q[0]<=q[1] and (last is None or all(a<=b for a,b in zip(last,q)));last=q
                assert row['type'] in {'observed','barrier'}
                values.append(row)
        assert dict(bytes=size,records=len(values),sha256=digest.hexdigest())==receipt[key]
        assert len(values)==source_receipt[key+'Rows']
        stats=receipt[key+'Stats'];assert stats['overflow'] is False and type(stats['samples']) is int and stats['samples']>=len(values)>0
        gaps=[max(0,decimal(b['q0'])-decimal(a['q1'])) for a,b in zip(values,values[1:])]
        assert stats['maxGapQpc']>=0 and stats['maxBracketQpc']>=max(decimal(row['q1'])-decimal(row['q0']) for row in values)
        # Cadence observes sampler attempts, including unpublished ones; retained rows are not all attempts.
        assert 0<=stats['firstQpc']<=decimal(values[0]['q0']) and stats['lastQpc']>=decimal(values[-1]['q1'])
        cadence[key]=dict(measured=stats,retainedMaxGapQpc=max(gaps,default=0));lanes[key]=values;total_bytes+=size;total_records+=len(values)
    assert total_bytes==receipt['combinedBytes'] and total_records==receipt['combinedRecords']
    poses=[row for row in lanes['pose'] if row['type']=='observed' and eligible(row,clock)]
    inputs=[row for row in lanes['input'] if row['type']=='observed' and eligible(row,clock)]
    assert poses and inputs
    baseline=poses[0]['before'];assert baseline['hwnd']==str(hwnd) and baseline['pid']==pid
    assert baseline['clientOrigin']==placement['geometry']['clientOrigin'] and baseline['clientSize']==[cw,ch] and baseline['dwmRect']==placement['geometry']['dwm'] and baseline['tid']==placement['geometry']['thread']
    assert baseline['dpi']==96 and baseline['visible'] is True and baseline['minimized'] is False and baseline['observerPmv2'] is baseline['targetPmv2'] is True
    assert baseline['monitorCount']==1 and baseline['monitorRect'][:2]==[0,0]
    for row in poses:
        assert row['before']==row['after']==baseline,'static source geometry/owner changed'
        pointer=row['pointerBefore'];assert pointer==row['pointerAfter'] and pointer['physicalPosition']==pointer['cursorPosition']==point and pointer['flags']==1 and pointer['handle']==str(placement['cursorHandle'])
        assert pointer['rootHwnd']==str(hwnd) and pointer['hitPid']==pid and pointer['hitTid']==baseline['tid']
    for row in lanes['input']:
        if row['type']=='observed':assert row['left'] is row['right'] is False,'static input contains down or unknown button sample'
    for row in inputs:
        assert row['physicalPosition']==point and row['rootHwnd']==str(hwnd) and row['hitPid']==pid and row['hitTid']==baseline['tid']
    resources_path=safe_file(root,session/'cursor/resources.json',65536);spool=resources_path.read_bytes()
    assert len(spool)==receipt['resourcesBytes'] and hashlib.sha256(spool).hexdigest()==receipt['resourcesSha256']
    resources=load(resources_path);assert resources['schemaVersion']==1 and resources['shapes']==index['shapes'] and len(index['shapes'])==receipt['resourcesCount']==1,'unexpected static resource shapes'
    shapes=shape_paths(root,index);shape=index['shapes'][0]
    assert any(row['shapeId']==shape['id'] for row in poses),'matching resource is not referenced by a stable eligible pose'
    assert all(row['shapeId']==shape['id'] for row in poses),'unknown static shape reference'
    oracle=root/'native-raster-oracle';oracle_shape=read('native-raster-oracle/shape.json')
    assert oracle_shape==dict(path='cursor-0.pam',width=16,height=16,hotspot=[3,5],phase='unknown')
    for name in ['cursor-0.pam']+[f'raster-0-{i}.pam' for i in range(4)]:safe_file(root,oracle/name,9*1024*1024)
    expected=cursor_check.raster(oracle,0,oracle_shape)
    assert cursor_check.pam(shapes[0])[2]==expected and [shape['width'],shape['height'],shape['hotspotX'],shape['hotspotY']]==[16,16,3,5]
    source=manifest['source'];raw=[source['rawSize'][k] for k in ['width','height']];encoded=[source['encodedSize'][k] for k in ['width','height']]
    assert source['backend']=='windowsWgcOwned' and source['kind']=='window' and source['windowId']==str(hwnd) and source['pointerOrigin'] is None
    assert [index['width'],index['height']]==encoded
    reference=raw_static(session,raw,encoded,geometry,baseline['dwmRect'])
    assert baseline['clientOrigin']==[reference['origin'][i]+reference['clientOffset'][i] for i in range(2)]
    sampled=read('production-sampled-interactions.json',8*1024*1024)
    interactions=verify_interactions(lanes['pose'],lanes['input'],index,sampled)
    assert sampled['clickEvidence']==sampled['dragEvidence']==sampled['compatibility']['clicks']==sampled['compatibility']['drags']==[]
    for name in ['clicks','drags','timeline']:assert read('session-0/'+name+'.json',8*1024*1024)==[]
    project=read('session-0/project.json',4*1024*1024)
    assert project==state['initialProject'],'ordinary import changed initial project'
    assert project['video']==dict(path='screen.mp4',width=encoded[0],height=encoded[1],durationUs=index['durationUs'],cursor='separate')
    assert project['cursor']==dict(indexPath='cursor/index.json') and project['clicks']==project['zooms']==[] and project.get('audioTracks') is None and project.get('microphone') is None
    preservation=read('session-0/media-preservation.json');movie=safe_file(root,session/'screen.mp4',64*1024*1024)
    assert preservation['unchanged'] is True and preservation['beforePublicationSha256']==preservation['afterImportSha256']==run.sha(movie),'import/media bytes changed'
    trace=read('session-0/trace.json',4*1024*1024);picks=trace_clock(trace,clock);assert [trace['width'],trace['height']]==raw
    startup=trace['startup'];assert startup['itemCreations']==startup['captureStarts']==startup['cleanupConsumptions']==1
    stages=[startup[k] for k in ['startQpc','freezeQpc','probeQpc','preparedQpc','gate2Qpc','encoderQpc','gate4Qpc','parkEnterQpc','parkLeaveQpc','supervisorQpc','writerQpc','armQpc']]
    assert all(type(x) is int and 0<=x<2**63 for x in stages) and stages==sorted(stages) and stages[-1]==trace['admission']
    assert all(startup[k]==0 for k in ['preArmPixelCopies','preAdmissionPixelCopies','preArmSourcePublications','preArmProofWrites'])
    assert startup['frozen']==raw and startup['pixelCopies']>=startup['sourcePublications']>=1
    sources=[e for e in trace['events'] if e['kind']=='source'];assert len(sources)==startup['sourcePublications'] and sources[0]['native']==trace['epoch']
    assert all(e['qpc']>=trace['admission'] and e['native']*trace['frequency']>=trace['admission']*10000000 for e in sources)
    result=check_static_media(root,ffmpeg,ffprobe,trace,index,geometry,reference)
    assert all(result.get(k) is True for k in ['decodedStaticContentChecked','decodedCursorFreePatchChecked','packetClockChecked'])
    result.update(productionStaticCasesPassed=True,rawGeometryChecked=True,rawCursorFreePatchChecked=True,resourceRasterChecked=True,traceClockChecked=True,cleanupChecked=True,importMediaPreservationChecked=True,independentOrigin=reference['origin'],eligiblePoseBrackets=len(poses),eligibleInputBrackets=len(inputs),sampledInteractions=interactions,cadence=cadence,finishMs=source_receipt['finishMs'],queueHighWater='unmeasured',productionCursorQualified=False,remaining='full resource/dynamic/input/load/forced-cleanup matrix and Task4 product parity; callback pixel identity, spatial/frame alignment and animation phase unmeasured')
    (root/'production-analysis.json').write_text(json.dumps(result,indent=2)+'\n')
    return result


def shape_paths(root,index):
    shapes=index['shapes'];assert len(shapes)==1,'unexpected static shape count'
    paths=[];decoded=0
    for number,shape in enumerate(shapes):
        assert shape['id']==f'shape-{number}' and shape['path']==f'cursor/shapes/shape-{number}.pam','unsafe/unknown shape path'
        assert all(type(shape[k]) is int for k in ['width','height','hotspotX','hotspotY'])
        assert [shape['width'],shape['height'],shape['hotspotX'],shape['hotspotY']]==[16,16,3,5]
        decoded+=shape['width']*shape['height']*4;assert decoded<=32*1024*1024
        path=safe_file(root,root/'session-0'/shape['path'],9*1024*1024)
        assert cursor_check.pam(path)[:2]==(shape['width'],shape['height']),'resource PAM dimensions disagree'
        paths.append(path)
    return paths


if __name__=='__main__':
    import sys
    assert len(sys.argv)==4,'usage: production_cursor_check.py OWNED_FIXTURE_ROOT FFMPEG FFPROBE'
    print(json.dumps(check(*map(lambda value:Path(value).resolve(),sys.argv[1:])),indent=2))
