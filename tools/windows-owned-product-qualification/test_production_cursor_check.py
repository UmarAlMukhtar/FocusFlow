"""Synthetic checker unit tests; no capture, native input or device calls."""
import unittest
import production_cursor_check as check

class Policy(unittest.TestCase):
    def test_exact_non_10mhz_half_open_and_width_boundaries(self):
        clock=dict(qpcFrequency='3000000',epoch100ns='10000000',admissionQpc='2997000',cutoffQpc='6000000',completedFrames=30)
        self.assertTrue(check.eligible(dict(q0='3000000',q1='3100000'),clock))
        for a,b in [(2999999,3000001),(5999999,6000000),(3000000,3100001),(3000001,3000000)]:
            self.assertFalse(check.eligible(dict(q0=str(a),q1=str(b)),clock))
    def test_fixed_dwm_model_is_never_selected_by_dimension_or_fitted(self):
        raw=[[8,31],[314,31],[8,264]]; physical=[[55,71],[361,71],[55,304]]
        self.assertEqual(check.model_origin(raw,physical,[47,40,369,312]),[47,40])
        with self.assertRaises(AssertionError):check.model_origin(raw,physical,[40,40,376,319])
        with self.assertRaises(AssertionError):check.model_origin(raw,[[55,71],[362,71],[55,304]],[47,40,369,312])

class InteractionEvidence(unittest.TestCase):
    def test_both_button_release_points_use_actual_rows_and_exact_brackets(self):
        import copy
        clock=dict(qpcFrequency='3000000',epoch100ns='10000000',admissionQpc='2997000',cutoffQpc='6000000',completedFrames=30)
        window=dict(hwnd='42',pid=7,tid=9,dwmRect=[47,40,369,312])
        poses=[dict(type='observed',q0='3000000',q1='3000001',before=window,after=window,barrierRevision=0)]
        inputs=[]
        for q,down,p in [(3000010,False,[50,50]),(3000030,True,[51,51]),(3060030,True,[63,51]),(3090030,False,[63,51])]:
            inputs.append(dict(type='observed',q0=str(q),q1=str(q+1),left=down,right=down,buttonsSwapped=False,basisPoseRow=0,barrierRevisionBefore=0,barrierRevisionAfter=0,rootHwnd='42',hitPid=7,hitTid=9,physicalPosition=p))
        clicks=[dict(button=button,previousInputRow=0,observedInputRow=1,basisPoseRow=0,occurrenceQpc=[3000010,3000031]) for button in ['left','right']]
        drags=[dict(button=edge['button'],startEdge=edge,inputPointRows=[1,3],endKind='observedRelease',releaseBracket=[3060030,3090031]) for edge in clicks]
        compat=[dict(points=[dict(x=4,y=11,timestamp=0.000011),dict(x=16,y=11,timestamp=0.030011)]) for _ in clicks]
        evidence=dict(truncated=False,clickEvidence=clicks,dragEvidence=drags,compatibility=dict(drags=compat))
        result=check.verify_interactions(poses,inputs,dict(clock=clock),evidence)
        self.assertEqual(result,dict(clicks=2,drags=2,buttons=['left','right'],endKinds=['observedRelease']))
        bad=copy.deepcopy(evidence);bad['dragEvidence'][0]['endKind']='interrupted'
        with self.assertRaises(AssertionError):check.verify_interactions(poses,inputs,dict(clock=clock),bad)
        bad=copy.deepcopy(evidence);bad['compatibility']['drags'][0]['points'][1]['timestamp']=0.020011
        with self.assertRaises(AssertionError):check.verify_interactions(poses,inputs,dict(clock=clock),bad)

if __name__=='__main__':unittest.main()

class OwnedMedia(unittest.TestCase):
    def test_bounded_stdout_stderr_and_input_without_pipe_deadlock(self):
        import sys
        receipts=[]
        self.assertTrue(hasattr(check,'media_output'),'bounded media output helper missing')
        code="import sys;sys.stderr.buffer.write(b'e'*200000);sys.stderr.flush();data=sys.stdin.buffer.read();sys.stdout.buffer.write(data)"
        data=b'x'*200000
        got=b''.join(check.media_output([sys.executable,'-c',code],len(data),receipts,input_bytes=data))
        self.assertEqual(got,data);self.assertEqual(receipts[0]['stderrBytes'],200000)
        self.assertLessEqual(len(receipts[0]['stderr']),65536)
        self.assertTrue(receipts[0]['reaped']);self.assertEqual(receipts[0]['exitCode'],0)

    def test_timeout_and_output_cap_reap_only_owned_child(self):
        import sys
        self.assertTrue(hasattr(check,'media_output'),'bounded media output helper missing')
        for code,cap,timeout in [("import time;time.sleep(10)",100,.05),("import sys;sys.stdout.buffer.write(b'x'*50000)",100,2)]:
            receipts=[]
            with self.assertRaises((AssertionError,TimeoutError)):
                list(check.media_output([sys.executable,'-c',code],cap,receipts,timeout=timeout))
            self.assertTrue(receipts[0]['reaped']);self.assertIsNotNone(receipts[0]['exitCode'])

    def test_consumer_failure_reaps_decoder_and_short_extra_frames_reject(self):
        import sys,contextlib
        self.assertTrue(hasattr(check,'media_frames'),'bounded frame reader missing')
        for count in [1,3]:
            receipts=[]
            with self.assertRaisesRegex(AssertionError,'frame|cap'):
                list(check.media_frames([sys.executable,'-c',f"import sys;sys.stdout.buffer.write(b'x'*{count*12})"],12,2,receipts))
            self.assertTrue(receipts[0]['reaped'])
        receipts=[]
        with contextlib.closing(check.media_frames([sys.executable,'-c',"import sys,time;sys.stdout.buffer.write(b'x'*12);sys.stdout.flush();time.sleep(10)"],12,1,receipts)) as frames:
            self.assertEqual(next(frames),b'x'*12)
        self.assertTrue(receipts[0]['reaped']);self.assertTrue(receipts[0]['killed'])

# All pixels and receipt rows below are synthetic checker inputs, never native proof.
def static_pixels():
    from test_analyze import Contract
    cw,ch=320,240;w,h=336,272;ox,oy=8,24
    rgb=bytearray([72]*(w*h*3));client=Contract().image(1,701,cw,ch)
    for y in range(ch):rgb[((y+oy)*w+ox)*3:((y+oy)*w+ox+cw)*3]=client[y*cw*3:(y+1)*cw*3]
    rgba=bytearray()
    for i in range(w*h):rgba.extend(rgb[i*3:i*3+3]+b'\xff')
    geometry=dict(physicalAnchors=[[55,71],[359,71],[55,295]],initialPointerPosition=[207,183],clientSize=[320,240],nonce=701)
    return w,h,bytes(rgba),bytes(rgb),geometry

def pam_file(path,w,h,pixels):
    path.parent.mkdir(parents=True,exist_ok=True)
    path.write_bytes(f'P7\nWIDTH {w}\nHEIGHT {h}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n'.encode()+pixels)

class StaticPixels(unittest.TestCase):
    def test_both_raw_exemplars_reject_leakage_and_duplicate_shifted_anchors(self):
        import tempfile
        from pathlib import Path
        self.assertTrue(hasattr(check,'raw_static'),'raw pair checker missing')
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);w,h,pixels,rgb,geometry=static_pixels()
            for i in range(2):pam_file(root/f'raw-{i}.pam',w,h,pixels)
            reference=check.raw_static(root,[w,h],[w,h],geometry,[39,39,w+39,h+39])
            self.assertEqual(reference['clientOffset'],[8,24]);self.assertEqual(reference['origin'],[39,39])
            for mutation in ['cursor','duplicate','shift']:
                bad=bytearray(pixels)
                if mutation=='cursor':bad[(144*w+165)*4:(144*w+165)*4+4]=bytes([0,0,0,255])
                if mutation=='duplicate':
                    for y in range(110,126):
                        for x in range(220,236):bad[(y*w+x)*4:(y*w+x)*4+4]=bytes([40,40,40,255])
                if mutation=='shift':geometry['physicalAnchors'][1][0]+=1
                pam_file(root/'raw-1.pam',w,h,bad)
                with self.subTest(mutation=mutation),self.assertRaises(AssertionError):check.raw_static(root,[w,h],[w,h],geometry,[39,39,w+39,h+39])

    def test_decorated_markers_use_measured_client_offset_and_every_replica(self):
        self.assertTrue(hasattr(check,'decoded_static'),'decoded marker checker missing')
        w,h,pixels,rgb,geometry=static_pixels()
        reference=dict(clientOffset=[8,24],anchors=[[16,32],[320,32],[16,256]])
        check.decoded_static(rgb,w,h,geometry,reference,701)
        for reference2,nonce,damage in [(dict(reference,clientOffset=[0,0]),701,False),(reference,702,False),(reference,701,True)]:
            bad=bytearray(rgb)
            if damage:bad[((24+92)*w+8+20)*3:((24+92)*w+8+20)*3+3]=b'\x80'*3
            with self.subTest(nonce=nonce,damage=damage),self.assertRaises(AssertionError):check.decoded_static(bad,w,h,geometry,reference2,nonce)

class StaticClock(unittest.TestCase):
    def test_trace_frequency_epoch_admission_cutoff_and_count_must_join(self):
        import copy
        from test_analyze import Contract
        self.assertTrue(hasattr(check,'trace_clock'),'static clock join missing')
        trace=Contract().trace();clock=dict(qpcFrequency='3000000',admissionQpc='3000000',epoch100ns='10000000',cutoffQpc='3200000',completedFrames=2,workerExitCode=0,encoderExitCode=0)
        self.assertEqual(len(check.trace_clock(trace,clock)),2)
        for field in clock:
            bad=copy.deepcopy(clock);bad[field]=str(int(bad[field])+1) if type(bad[field]) is str else bad[field]+1
            with self.subTest(field=field),self.assertRaises(AssertionError):check.trace_clock(trace,bad)
        for field in ['frequency','admission','epoch','cutoff','selected','written']:
            bad=copy.deepcopy(trace);bad[field]+=1
            with self.subTest(field=field),self.assertRaises(AssertionError):check.trace_clock(bad,clock)

    def test_saved_stream_packet_endpoint_and_foreign_audio_rejected(self):
        import copy
        self.assertTrue(hasattr(check,'saved_grid'),'saved stream checker missing')
        meta=dict(streams=[dict(index=0,codec_type='video',codec_name='h264',pix_fmt='yuv420p',width=336,height=272,time_base='1/30000',duration_ts=2000,start_pts=0)],packets=[dict(stream_index=0,pts=0,dts=0,duration=1000),dict(stream_index=0,pts=1000,dts=1000,duration=1000)])
        check.saved_grid(meta,[336,272],2)
        for mutate in [lambda m:m['streams'].append(dict(codec_type='audio')),lambda m:m['packets'][1].update(duration=999),lambda m:m['packets'][0].update(stream_index=1),lambda m:m['streams'][0].update(duration_ts=2001),lambda m:m['streams'][0].update(color_space='bt709'),lambda m:m['streams'][0].update(width=335)]:
            bad=copy.deepcopy(meta);mutate(bad)
            with self.assertRaises(AssertionError):check.saved_grid(bad,[336,272],2)

def synthetic_case(parent):
    import json,hashlib,copy
    from test_analyze import Contract
    root=parent/'production-decorated-even-r0';session=root/'session-0';session.mkdir(parents=True)
    def put(name,value):
        path=root/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(value))
    w,h,pixels,rgb,geometry=static_pixels();geometry.pop('nonce');profile='pmv2_96_single_monitor_decorated';geometry['profile']=profile
    clock=dict(qpcFrequency='3000000',admissionQpc='3000000',epoch100ns='10000000',cutoffQpc='3200000',completedFrames=2,workerExitCode=0,encoderExitCode=0)
    window=dict(hwnd='42',pid=22,tid=23,dwmRect=[39,39,375,311],clientOrigin=[47,63],clientSize=[320,240],windowRect=[39,39,375,311],dpi=96,visible=True,minimized=False,observerPmv2=True,targetPmv2=True,monitorCount=1,monitorRect=[0,0,1920,1080])
    pointer=dict(physicalPosition=[207,183],cursorPosition=[207,183],flags=1,handle='55',rootHwnd='42',hitHwnd='42',hitPid=22,hitTid=23)
    pose=dict(type='observed',q0='3000010',q1='3000011',before=window,after=window,pointerBefore=pointer,pointerAfter=pointer,shapeId='shape-0',barrierRevision=0)
    input=dict(type='observed',q0='3000020',q1='3000021',physicalPosition=[207,183],rootHwnd='42',hitHwnd='42',hitPid=22,hitTid=23,left=False,right=False,buttonsSwapped=False,basisPoseRow=0,barrierRevisionBefore=0,barrierRevisionAfter=0)
    shape=dict(id='shape-0',path='cursor/shapes/shape-0.pam',width=16,height=16,hotspotX=3,hotspotY=5)
    terminal=dict(poseBarrier=None,inputBarrier=None,sourceCutoffQpc=3200000,stopKind='stop')
    index=dict(schemaVersion=2,width=w,height=h,durationUs=66666,observationsPath='cursor/observations.ndjson',inputObservationsPath='cursor/input-observations.ndjson',shapes=[shape],clock=clock,model='dwmExtendedFrameTranslationV1',profile=profile,terminal=terminal,spatialErrorPx=None,frameAlignmentErrorUs=None,animationPhase='unknown')
    put('session-0/cursor/index.json',index);put('session-0/cursor/resources.json',dict(schemaVersion=1,shapes=[shape]))
    resource=(session/'cursor/resources.json').read_bytes()
    receipt=dict(profile=profile,terminal=terminal,poseJoined=True,inputJoined=True,filesUsable=True,degraded=False,resourcesBytes=len(resource),resourcesSha256=hashlib.sha256(resource).hexdigest(),resourcesCount=1)
    for name,key,row in [('observations.ndjson','pose',pose),('input-observations.ndjson','input',input)]:
        data=(json.dumps(row)+'\n').encode();(session/'cursor'/name).write_bytes(data)
        receipt[key]=dict(bytes=len(data),records=1,sha256=hashlib.sha256(data).hexdigest())
        receipt[key+'Stats']=dict(samples=1,maxGapQpc=0,maxBracketQpc=1,firstQpc=int(row['q0']),lastQpc=int(row['q1']),overflow=False)
    receipt.update(combinedBytes=receipt['pose']['bytes']+receipt['input']['bytes'],combinedRecords=2)
    put('production-observer-completion.json',receipt);put('production-geometry.json',geometry)
    placement=dict(hwnd=42,sourcePid=22,clientSize=[320,240],position=[207,183],cursorHandle=55,cursorFlags=1,buttonsUp=True,footprint=[204,178,16,16],margin=2,geometry=dict(clientOrigin=[47,63],clientSize=[320,240],window=window['windowRect'],dwm=window['dwmRect'],dpi=96,pid=22,thread=23),operations=[dict(operation='place-center',q0=2900000,q1=2900010),dict(operation='wake-horizontal',dx=1,q0=2900020,q1=2900030),dict(operation='wake-horizontal',dx=-1,q0=2900040,q1=2900050),dict(operation='settle-center',q0=2900060,q1=2999999)],wakeMs=100,settleMs=150)
    put('production-placement.json',placement)
    for name,value in [('source-ready',dict(pid=22,hwnd=42)),('source-observed',dict(pid=22,hwnd=42)),('source-selection',dict(driverPid=11,sourcePid=22,hwnd=42)),('source-request',dict(width=320,height=240,nonce=701,animated=False,privateProfile='decorated')),('source-cleanup',dict(pid=22,observed=True,signaled=True,exitCode=0,fallback=False,error=None)),('source-exit',dict(exitCode=0))]:put('source/'+name+'.json',value)
    for name,value in [('production-source-ready',dict(hwnd=42,sourcePid=22)),('production-start-authorized',dict(action='static-owned-canvas-v1',hwnd=42,sourcePid=22)),('production-armed',dict(hwnd=42,sourcePid=22,profile=profile,candidateOnly=True)),('production-actions-complete',dict(action='static-hold-complete-v1',hwnd=42,sourcePid=22)),('production-phase',dict(phase='postcapture',hwnd=42,sourcePid=22,workerEncoderJoined=True,sourceClosed=True,qpc=3300000)),('production-source-receipt',dict(profile=profile,clock=clock,terminal=terminal,poseRows=1,inputRows=1,finishMs=1,ordinaryImport=True,productionCursorQualified=False,independentPixelAndInputOracleRequired=True))]:put(name+'.json',value)
    owner=dict(parent=11,worker=33,encoder=44,qpc=2999000);put('session-0/owned.json',owner)
    source=dict(pid=22,observed=True,signaled=True,exitCode=0)
    command=dict(error=None,exitCode=0,cleanupUnknown=False,cleanup=dict(parent=dict(pid=11,signaled=True,exitCode=0),owned=[dict(owner,path='session-0/owned.json',signaled=[True,True])],sources=[source]),ownership=[dict(owner,signaled=[True,True],fallback=False)],productionStaticPhase=dict(phase='postcapture'))
    (parent/(root.name+'-command.json')).write_text(json.dumps(command))
    put('session-0/cleanup.json',dict(workerSignaled=True,encoderSignaled=True,exactNoninheritableHandles=True,qpc=3250000))
    project=dict(video=dict(path='screen.mp4',width=w,height=h,durationUs=66666,cursor='separate'),cursor=dict(indexPath='cursor/index.json'),clicks=[],zooms=[],audioTracks=None,microphone=None)
    state=dict(status='finalized',videoTiming=dict(clock=clock,endpoint=dict(endPts=2,timeBase=dict(numerator=1,denominator=30),interval='halfOpen',provenance='savedPacketFullGrid')),initialProject=project)
    manifest=dict(schemaVersion=3,captureId='session-0',state=state,source=dict(backend='windowsWgcOwned',kind='window',windowId='42',rawSize=dict(width=w,height=h),encodedSize=dict(width=w,height=h),pointerOrigin=None))
    put('session-0/capture.json',manifest);put('session-0/project.json',project);(session/'capture.windows-v3').write_bytes(b'focusflow-windows-capture-v3\n')
    (session/'screen.mp4').write_bytes(b'SYNTHETIC checker media placeholder')
    digest=hashlib.sha256((session/'screen.mp4').read_bytes()).hexdigest();put('session-0/media-preservation.json',dict(beforePublicationSha256=digest,afterImportSha256=digest,unchanged=True))
    interactions=dict(truncated=False,clickEvidence=[],dragEvidence=[],compatibility=dict(clicks=[],drags=[]))
    put('production-sampled-interactions.json',interactions)
    for name in ['clicks','drags','timeline']:put('session-0/'+name+'.json',[])
    trace=Contract().trace();trace.update(width=w,height=h)
    for event in trace['events']:
        if event['kind']=='source':event.update(contentWidth=w,contentHeight=h,textureWidth=w,textureHeight=h,paint=0,nonce=0,corners=[0,0,0,0])
    # Production publishes its first admitted source as seq0; no pre-admission source pixel copies.
    trace['events']=trace['events'][1:]
    for i,event in enumerate(trace['events']):event['order']=i
    for write in trace['writes']:write['bytes']=w*h*4
    trace['startup']=dict(itemCreations=1,captureStarts=1,cleanupConsumptions=1,pixelCopies=2,sourcePublications=2,preArmPixelCopies=0,preAdmissionPixelCopies=0,preArmSourcePublications=0,preArmProofWrites=0,proofWrites=0,metadataCallbacks=3,mismatches=0,allocation=[w,h],frozen=[w,h],**{k:2900000 for k in ['startQpc','freezeQpc','probeQpc','preparedQpc','gate2Qpc','encoderQpc','gate4Qpc','parkEnterQpc','parkLeaveQpc','supervisorQpc','writerQpc']},armQpc=3000000)
    put('session-0/trace.json',trace)
    for i in range(2):pam_file(session/f'raw-{i}.pam',w,h,pixels)
    cursor=bytearray()
    for y in range(16):
        for x in range(16):
            opaque=(x<4 and y<12) or (y>8 and x<12);c=255 if opaque and x<2 else 0;cursor.extend([c,c,c,255 if opaque else 0])
    pam_file(session/shape['path'],16,16,cursor);oracle=root/'native-raster-oracle'
    pam_file(oracle/'cursor-0.pam',16,16,cursor)
    put('native-raster-oracle/shape.json',dict(path='cursor-0.pam',width=16,height=16,hotspot=[3,5],phase='unknown'))
    for bg in range(4):
        raster=bytearray()
        for i in range(256):
            x,y=i%16,i//16;background=([0]*3 if bg==0 else [255]*3 if bg==1 else [35 if (x//4+y//4)%2==0 else 210]*3 if bg==2 else [37,91,173]);p=cursor[i*4:i*4+4];raster.extend(list(p[:3] if p[3] else background)+[255])
        pam_file(oracle/f'raster-0-{bg}.pam',16,16,raster)
    return root

class StaticIntegration(unittest.TestCase):
    def test_actual_v3_nested_state_and_named_static_result(self):
        import tempfile
        from pathlib import Path
        from unittest.mock import patch
        self.assertIn('ffmpeg',__import__('inspect').signature(check.check).parameters,'static checker not integrated')
        with tempfile.TemporaryDirectory() as d:
            root=synthetic_case(Path(d))
            with patch.object(check,'check_static_media',return_value=dict(decodedStaticContentChecked=True,decodedCursorFreePatchChecked=True,packetClockChecked=True)):
                result=check.check(root,Path('fake-decoder'),Path('fake-probe'))
            self.assertTrue(result['productionStaticCasesPassed']);self.assertFalse(result['productionCursorQualified']);self.assertNotIn('passed',result)
            self.assertEqual(result['queueHighWater'],'unmeasured')

    def test_missing_join_degraded_changed_owner_unused_shape_down_input_and_import_fail(self):
        import tempfile,json
        from pathlib import Path
        from unittest.mock import patch
        self.assertIn('ffmpeg',__import__('inspect').signature(check.check).parameters,'static checker not integrated')
        cases=[('production-observer-completion.json',lambda x:x.update(poseJoined=False)),('production-observer-completion.json',lambda x:x.update(degraded=True)),('production-source-receipt.json',lambda x:x.update(ordinaryImport=False)),('production-source-ready.json',lambda x:x.update(sourcePid=99)),('production-geometry.json',lambda x:x['physicalAnchors'][1].__setitem__(0,360)),('session-0/cursor/index.json',lambda x:x['shapes'].append(dict(x['shapes'][0],id='unused'))),('session-0/cursor/observations.ndjson',lambda x:x.update(shapeId=None)),('session-0/cursor/observations.ndjson',lambda x:x['before'].update(pid=99)),('session-0/cursor/input-observations.ndjson',lambda x:x.update(left=True)),('production-sampled-interactions.json',lambda x:x['compatibility']['clicks'].append(dict(x=1,y=1))),('session-0/project.json',lambda x:x['video'].update(cursor='embedded')),('session-0/media-preservation.json',lambda x:x.update(afterImportSha256='0'*64)),('session-0/cleanup.json',lambda x:x.update(encoderSignaled=False)),('session-0/capture.json',lambda x:x['state']['videoTiming']['endpoint'].update(endPts=3))]
        for name,mutate in cases:
            with self.subTest(file=name),tempfile.TemporaryDirectory() as d:
                root=synthetic_case(Path(d));path=root/name;data=json.loads(path.read_text());mutate(data);path.write_text(json.dumps(data)+'\n')
                if name.endswith('.ndjson'):
                    import hashlib
                    receipt_path=root/'production-observer-completion.json';receipt=json.loads(receipt_path.read_text());key='input' if 'input-' in name else 'pose';raw=path.read_bytes()
                    receipt[key].update(bytes=len(raw),sha256=hashlib.sha256(raw).hexdigest());receipt['combinedBytes']=receipt['input']['bytes']+receipt['pose']['bytes'];receipt_path.write_text(json.dumps(receipt))
                with patch.object(check,'check_static_media',return_value=dict(decodedStaticContentChecked=True,decodedCursorFreePatchChecked=True,packetClockChecked=True)):
                    with self.assertRaises(AssertionError):check.check(root,Path('fake'),Path('fake'))

class StaticMediaIntegration(unittest.TestCase):
    def test_all_decoded_frames_and_exact_count_are_checked(self):
        import tempfile,json
        from pathlib import Path
        from unittest.mock import patch
        for corruption in [None,'late-marker','short','extra','late-cursor']:
            with self.subTest(corruption=corruption),tempfile.TemporaryDirectory() as d:
                root=synthetic_case(Path(d));w,h,pixels,rgb,geometry=static_pixels()
                trace=json.loads((root/'session-0/trace.json').read_text());index=json.loads((root/'session-0/cursor/index.json').read_text())
                reference=check.raw_static(root/'session-0',[w,h],[w,h],geometry,[39,39,375,311])
                planar=bytes([96])*(w*h)+bytes([128])*(w*h//2)
                meta=dict(streams=[dict(index=0,codec_type='video',codec_name='h264',pix_fmt='yuv420p',width=w,height=h,time_base='1/30000',duration_ts=2000,start_pts=0)],packets=[dict(stream_index=0,pts=0,dts=0,duration=1000),dict(stream_index=0,pts=1000,dts=1000,duration=1000)])
                def output(argv,limit,receipts,input_bytes=None,timeout=20):
                    if '-version' in argv:yield b'synthetic media tool\n';return
                    if '-show_streams' in argv:yield json.dumps(meta).encode();return
                    if input_bytes is not None:yield planar;return
                    is_rgb='rgb24' in argv;base=rgb if is_rgb else planar
                    count=1 if corruption=='short' else 3 if corruption=='extra' else 2
                    for ordinal in range(count):
                        data=bytearray(base)
                        if ordinal==1 and corruption=='late-marker' and is_rgb:
                            at=((24+44)*w+8+20)*3;data[at:at+3]=b'\x80'*3
                        if ordinal==1 and corruption=='late-cursor' and not is_rgb:
                            data[144*w+165]=255
                        for offset in range(0,len(data),4096):yield bytes(data[offset:offset+4096])
                # Only external tool bytes are replaced; saved metadata, frame iteration and all oracles run.
                with patch.object(check,'media_output',side_effect=output),patch('run.sha',return_value='synthetic-tool-hash'):
                    if corruption:
                        with self.assertRaises(AssertionError):check.check_static_media(root,Path('fake'),Path('fake-probe'),trace,index,geometry,reference)
                    else:
                        result=check.check_static_media(root,Path('fake'),Path('fake-probe'),trace,index,geometry,reference)
                        self.assertEqual(result['decodedFrames'],2);self.assertTrue(result['decodedStaticContentChecked']);self.assertTrue(result['decodedCursorFreePatchChecked'])

    def test_noncollinear_anchor_requirement(self):
        with self.assertRaisesRegex(AssertionError,'collinear'):check.model_origin([[1,1],[2,2],[3,3]],[[5,5],[6,6],[7,7]],[4,4,50,50])

class ResourceDimensions(unittest.TestCase):
    def test_matching_shape_bytes_cannot_hide_wrong_pam_dimensions(self):
        import tempfile,json
        from pathlib import Path
        with tempfile.TemporaryDirectory() as d:
            root=synthetic_case(Path(d));path=root/'session-0/cursor/shapes/shape-0.pam'
            path.write_bytes(path.read_bytes().replace(b'WIDTH 16\nHEIGHT 16',b'WIDTH 8\nHEIGHT 32'))
            with self.assertRaises(AssertionError):check.shape_paths(root,json.loads((root/'session-0/cursor/index.json').read_text()))

class SourceCleanupJoin(unittest.TestCase):
    def test_selected_source_requires_same_normal_cleanup_owner(self):
        import tempfile,json
        from pathlib import Path
        from unittest.mock import patch
        import run
        for cleanup_pid in [22,99]:
            with self.subTest(cleanup_pid=cleanup_pid),tempfile.TemporaryDirectory() as d:
                root=synthetic_case(Path(d));command=root.parent/(root.name+'-command.json');saved=root/'source/source-cleanup.json'
                data=json.loads(command.read_text());data['cleanup']['sources'][0]['pid']=cleanup_pid;command.write_text(json.dumps(data))
                data=json.loads(saved.read_text());data['pid']=cleanup_pid;saved.write_text(json.dumps(data))
                # Both cleanup copies remain mutually consistent; only their join to selected PID22 differs.
                run.require_production_cleanup(root,command,complete=True)
                with patch.object(check,'check_static_media',return_value=dict(decodedStaticContentChecked=True,decodedCursorFreePatchChecked=True,packetClockChecked=True)):
                    if cleanup_pid==22:
                        result=check.check(root,Path('fake-decoder'),Path('fake-probe'))
                        self.assertTrue(result['productionStaticCasesPassed']);self.assertTrue(result['cleanupChecked']);self.assertFalse(result['productionCursorQualified'])
                    else:
                        with self.assertRaisesRegex(AssertionError,'source.*cleanup|cleanup.*source'):check.check(root,Path('fake-decoder'),Path('fake-probe'))
