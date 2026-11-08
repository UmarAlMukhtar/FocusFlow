"""Private lane routing and independent mathematical/resource trust boundaries; no native calls."""
import os
from pathlib import Path
import unittest
from unittest.mock import patch
import run
from test_run import STARTUP_UNIT_LOGS

class CursorQualification(unittest.TestCase):
    def test_positive_hosted_binding_before_any_mutation(self):
        # The existing lane allows absent GITHUB_REF. The private mutation lane must not.
        good = dict(GITHUB_ACTIONS='true', RUNNER_ENVIRONMENT='github-hosted', RUNNER_OS='Windows',
                    GITHUB_REPOSITORY='UmarAlMukhtar/FocusFlow', GITHUB_REF='refs/heads/codex-windows-cursor-qualification',
                    GITHUB_EVENT_NAME='push', GITHUB_WORKFLOW_REF='UmarAlMukhtar/FocusFlow/.github/workflows/windows-owned-product-qualification.yml@refs/heads/codex-windows-cursor-qualification', GITHUB_SHA='a'*40, GITHUB_RUN_ID='123', GITHUB_RUN_ATTEMPT='1',
                    FOCUSFLOW_CURSOR_REVIEWED_SHA='a'*40, FOCUSFLOW_CURSOR_REVIEWED_REPOSITORY='UmarAlMukhtar/FocusFlow',
                    FOCUSFLOW_CURSOR_REVIEWED_REF='refs/heads/codex-windows-cursor-qualification',
                    FOCUSFLOW_CURSOR_REVIEWED_RUN='123:1')
        with patch.dict(os.environ,good,clear=True):
            check = getattr(run, 'require_cursor_disposable_context', lambda: None)
            self.assertIsNotNone(check(), 'baseline has no positive disposable-context handshake')
        for key in good:
            bad=good.copy();bad.pop(key)
            with self.subTest(key=key),patch.dict(os.environ,bad,clear=True):
                check=getattr(run,'require_cursor_disposable_context',lambda:None)
                with self.assertRaises(AssertionError):check()

if __name__=='__main__':unittest.main()

class PrivateChecks(unittest.TestCase):
    def test_independent_anchors_and_rational_boundary_negatives(self):
        import cursor_check as c
        raw=[[8,8],[633,8],[8,351]];physical=[[-92,-42],[533,-42],[-92,301]]
        self.assertEqual(c.unit_origin(raw,physical),[-100,-50])
        for bad in [[[9,8],[633,8],[8,351]],[[16,16],[1266,16],[16,702]],[[8,8],[9,9],[10,10]]]:
            with self.assertRaises(AssertionError):c.unit_origin(bad,physical)
        clock=dict(frequency=3000000,epoch=10000000,admission=2999000,cutoff=6000000,written=30)
        self.assertEqual(c.bracket(dict(q0=3000000,q1=3000006),clock)['atUs'],1)
        for q0,q1 in [(-1,0),(2999999,3000001),(5999999,6000001),(6000000,6000000),(3000000,3100001),(3000001,3000000)]:
            with self.assertRaises(AssertionError):c.bracket(dict(q0=q0,q1=q1),clock)
        # Actual rejected native interval remains rejected after acquisition/processing separation.
        native_clock=dict(frequency=10000000,epoch=5977187500,admission=5975549528,cutoff=5984583167,written=22)
        with self.assertRaisesRegex(AssertionError,'invalid/overwide QPC bracket'):
            c.bracket(dict(q0=5979106388,q1=5980388292),native_clock)

    def test_raw_upload_allowlist_includes_decisive_pams_and_rejects_authority_leaks(self):
        import tempfile
        from package_evidence import package
        import cursor_check as c
        workflow=(Path(__file__).resolve().parents[2]/'.github/workflows/windows-owned-product-qualification.yml').read_text()
        globs=[line.strip() for line in workflow.splitlines() if line.strip().startswith('qualification-upload/')]
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);ev=root/'evidence';case=ev/'cursor-borderless-even/session-0';d=case/'with';d.mkdir(parents=True)
            data=b'P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n'+bytes([1,2,3,255])
            for name in ['raw-0.pam','cursor-0.pam','raster-0-0.pam','native-color-0.pam','native-color-11.pam']:(d/name).write_bytes(data)
            (d/'proof-with-cursor.mp4').write_bytes(b'private');(d/'geometry.json').write_text('{}');(d/'raw-999.pam').write_bytes(b'not named')
            (case/'source-command-13.json').write_text('{}')
            probe=case/'alpha-probe';probe.mkdir()
            for name in ['cursor-10.pam','native-color-10.pam']+[f'raster-10-{i}.pam' for i in range(4)]:(probe/name).write_bytes(data)
            (probe/'native-color-12.pam').write_bytes(data)
            alpha=case/'alpha-with';alpha.mkdir();(alpha/'native-color-0.pam').write_bytes(data)
            (alpha/'native-color-12.pam').write_bytes(data);(case/'source-command-17.json').write_text('{}')
            resize=case/'resize';resize.mkdir()
            resize_names=['trace.json','proof-summary.json','owned.json','first-write-started.json','resize-observation.json','raw-0.pam','raw-1.pam','proof-with-cursor.mp4']
            for name in resize_names:(resize/name).write_bytes(data if name.endswith('.pam') else b'private' if name.endswith('.mp4') else b'{}')
            (resize/'private.json').write_text('not named');(resize/'nested').mkdir();(resize/'nested/trace.json').write_text('not named')
            hashes=package(ev,root/'absent',root/'qualification-upload')
            expected={f'cases/cursor-borderless-even/session-0/with/{n}' for n in ['raw-0.pam','cursor-0.pam','raster-0-0.pam','native-color-0.pam','native-color-11.pam','proof-with-cursor.mp4','geometry.json']}|{'cases/cursor-borderless-even/session-0/source-command-13.json','cases/cursor-borderless-even/session-0/alpha-with/native-color-0.pam'}
            expected|={f'cases/cursor-borderless-even/session-0/alpha-probe/{n}' for n in ['cursor-10.pam','native-color-10.pam']+[f'raster-10-{i}.pam' for i in range(4)]}
            expected|={f'cases/cursor-borderless-even/session-0/resize/{name}' for name in resize_names}
            self.assertEqual(set(hashes),expected)
            import hashlib,json
            manifest=json.loads((root/'qualification-upload/upload-manifest.json').read_text());self.assertEqual(manifest['sha256'],hashes);self.assertFalse(manifest['omitted'])
            for name,value in hashes.items():self.assertEqual(value,hashlib.sha256((root/'qualification-upload'/name).read_bytes()).hexdigest())
            uploaded={p.relative_to(root/'qualification-upload').as_posix() for glob in globs for p in root.glob(glob) if p.is_file()};self.assertTrue(expected<=uploaded)
            self.assertEqual(c.pam(d/'raw-0.pam'),(1,1,bytes([1,2,3,255])))
            (probe/'capture.json').write_text('{}')
            with self.assertRaises(AssertionError):c.private_only(case)
            with self.assertRaises(RuntimeError):package(ev,root/'absent',root/'probe-leak')
            (probe/'capture.json').unlink()
            (d/'capture.json').write_text('{}')
            with self.assertRaises(AssertionError):c.private_only(case)
            with self.assertRaises(RuntimeError):package(ev,root/'absent',root/'leak')
            (d/'capture.json').unlink();(d/'raw-0.pam').write_bytes(b'x'*(9*1024*1024+1))
            with self.assertRaises(AssertionError):run.bound(ev)
            with self.assertRaises(RuntimeError):package(ev,root/'absent',root/'cap')

    def test_private_movie_requires_exact_worker_and_encoder_completion(self):
        import json
        import tempfile
        import cursor_check as c

        accepted = dict(privateProof=True, neverImport=True, workerExit=0, encoderSignaled=True, clock={})
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            for key, value in [('privateProof', False), ('neverImport', False),
                               ('workerExit', 7), ('encoderSignaled', False)]:
                with self.subTest(key=key):
                    (directory/'proof-summary.json').write_text(json.dumps(accepted | {key: value}))
                    with self.assertRaisesRegex(AssertionError, 'Private proof completion'):
                        c.movie(directory, {}, None, None, [])

    def test_private_lifecycle_never_calls_embedded_arm_publish_or_legacy_names(self):
        repo=Path(__file__).resolve().parents[2]
        proof=(repo/'FocusFlow/src-tauri/src/recorder/windows_capture/process/qualification/proof.rs').read_text()
        # Structural guard supplements actual core import test and native private_only checker.
        self.assertNotIn('publish_windows_incomplete(',proof);self.assertNotIn('Completion::',proof);self.assertNotIn('Session::prepare(',proof);self.assertNotIn('StartReservation',proof)
        self.assertIn('manifest:None',proof);self.assertIn('proof-without-cursor.mp4',proof)
        production=(repo/'FocusFlow/src-tauri/src/recorder/windows_capture/process.rs').read_text()
        self.assertIn('#[cfg(not(test))] let cursor=CursorCaptureSettings::WithCursor;',production)

class PrivateRouting(unittest.TestCase):
    def test_private_scope_builds_one_harness_and_stops_on_unknown_cleanup(self):
        import json,tempfile
        from types import SimpleNamespace
        for fail in [False,True,'missing-resize','missing-observation']:
            with self.subTest(fail=fail),tempfile.TemporaryDirectory() as tmp:
                repo=Path(__file__).resolve().parents[2];root=Path(tmp);args=SimpleNamespace(evidence=root/'evidence',ffmpeg=root/'f',ffprobe=root/'p',scope='cursor-private')
                metadata={'packages':[{'name':'windows-capture','source':None,'manifest_path':str(repo/'FocusFlow/vendor/windows-capture/Cargo.toml')}]};calls=[];builds=[]
                def execute(argv,evidence,name,*unused,**kwargs):
                    calls.append(name)
                    if name in {'cursor-portable','cursor-renderer'}|STARTUP_UNIT_LOGS.keys():
                        log=evidence/(name+'.log');log.write_text(STARTUP_UNIT_LOGS.get(name,'test result: ok. '+('6' if name=='cursor-portable' else '1')+' passed; 0 failed; 0 ignored;'));return log
                    self.assertTrue(name.startswith('cursor-'));self.assertEqual(argv[2],'cursor-private')
                    if fail is True:raise RuntimeError('required first native profile failed')
                    case=evidence/name;receipt=evidence/(name+'-command.json');data=private_owner_fixture(case,receipt)
                    if fail=='missing-resize':
                        import shutil
                        shutil.rmtree(case/'session-0/resize');data['cleanup']['owned']=[row for row in data['cleanup']['owned'] if '/resize/' not in row['path']];data['ownership'].pop(5)
                    if fail=='missing-observation':data['ownership'].pop(5)
                    run.write(receipt,data)
                def build(repo,evidence,manifest,name,test):builds.append((name,test));return root/'harness'
                with patch('run.sys.platform','win32'),patch.dict(os.environ,{'GITHUB_REF':'refs/heads/codex-windows-cursor-qualification'}),patch('run.require_cursor_disposable_context',return_value={'sha':'a'*40}),patch('run.os.chdir'),patch('run.source_manifest',return_value={}),patch('run.sha',side_effect=lambda p:run.FFMPEG_SHA if p==args.ffmpeg else run.FFPROBE_SHA if p==args.ffprobe else 'fixture-hash'),patch('run.subprocess.check_output',side_effect=lambda argv,**kw:json.dumps(metadata) if argv[0]=='cargo' else 'a'*40),patch('run.execute',side_effect=execute),patch('run.build_exe',side_effect=build),patch('cursor_check.check') as checker:
                    if fail:
                        with self.assertRaises(RuntimeError):run.run(args)
                    else:run.run(args)
                self.assertEqual(builds,[('windows_owned_qualification',True)])
                self.assertEqual(len(calls),6 if fail is True else 9);self.assertEqual(checker.call_count,0 if fail else 4)
                result=json.loads((args.evidence/'result.json').read_text());self.assertFalse(result['passed']);self.assertFalse(result['productionCursorQualified']);self.assertEqual(result['cursorProofPassed'],not fail)
                self.assertNotIn('core-publication',calls);self.assertNotIn('native-platform',calls)

    def test_clean_profile_failure_collects_all_four_but_unknown_cleanup_stops(self):
        import json,tempfile
        from types import SimpleNamespace
        profiles=['cursor-borderless-even','cursor-decorated-even','cursor-borderless-odd','cursor-decorated-odd']
        faults=['confirmed','worker-live','encoder-live','source-live','parent-live','unknown-owner','missing-receipt','unobserved-preparation','owner-identity','source-identity']
        for cleanup,failed_profile in [(fault,0) for fault in faults]+[('confirmed',i) for i in [1,2,3]]+[(fault,None) for fault in ['worker-live','missing-receipt','unobserved-preparation']]:
            with self.subTest(cleanup=cleanup,failed_profile=failed_profile),tempfile.TemporaryDirectory() as tmp:
                repo=Path(__file__).resolve().parents[2];root=Path(tmp);args=SimpleNamespace(evidence=root/'evidence',ffmpeg=root/'f',ffprobe=root/'p',scope='cursor-private');calls=[]
                metadata={'packages':[{'name':'windows-capture','source':None,'manifest_path':str(repo/'FocusFlow/vendor/windows-capture/Cargo.toml')}]}
                def execute(argv,evidence,name,*unused,**kwargs):
                    log=evidence/(name+'.log');log.write_text(STARTUP_UNIT_LOGS.get(name,'test result: ok. '+('6' if name=='cursor-portable' else '1')+' passed; 0 failed; 0 ignored;'))
                    if name not in profiles:return log
                    calls.append((name,list(argv[3:6])));case=evidence/name/'session-0';d=case/'without';d.mkdir(parents=True)
                    owned=dict(parent=10,worker=20,encoder=30,qpc=123);run.write(d/'owned.json',owned)
                    source=dict(pid=40,observed=True,signaled=True,exitCode=0);run.write(case/'source-cleanup.json',source)
                    final=dict(parent=dict(pid=10,signaled=True,exitCode=0),owned=[dict(**owned,path='session-0/without/owned.json',signaled=[True,True])],sources=[source])
                    if cleanup=='worker-live':final['owned'][0]['signaled'][0]=False
                    if cleanup=='encoder-live':final['owned'][0]['signaled'][1]=False
                    if cleanup=='source-live':final['sources'][0]['signaled']=False
                    if cleanup=='parent-live':final['parent']['signaled']=False
                    if cleanup=='unknown-owner':run.write(case/'owned.json',owned)
                    if cleanup=='unobserved-preparation':(case/'with').mkdir()
                    if cleanup=='owner-identity':final['owned'][0]['worker']=99
                    if cleanup=='source-identity':run.write(case/'source-cleanup.json',dict(source,pid=99))
                    if cleanup!='missing-receipt':run.write(evidence/(name+'-command.json'),dict(argv=[str(x) for x in argv],exitCode=0,error=None,cleanup=final))
                    if failed_profile is not None and len(calls)==failed_profile+1:raise RuntimeError('profile failed after owned cleanup: '+name)
                    return log
                with patch('run.sys.platform','win32'),patch.dict(os.environ,{'GITHUB_REF':'refs/heads/codex-windows-cursor-qualification'}),patch('run.require_cursor_disposable_context',return_value={'sha':'a'*40}),patch('run.os.chdir'),patch('run.source_manifest',return_value={}),patch('run.sha',side_effect=lambda p:run.FFMPEG_SHA if p==args.ffmpeg else run.FFPROBE_SHA if p==args.ffprobe else 'fixture-hash'),patch('run.subprocess.check_output',side_effect=lambda argv,**kw:json.dumps(metadata) if argv[0]=='cargo' else 'a'*40),patch('run.execute',side_effect=execute),patch('run.build_exe',return_value=root/'harness'),patch('cursor_check.check'):
                    with self.assertRaises(RuntimeError):run.run(args)
                self.assertEqual([c[0] for c in calls],profiles if cleanup=='confirmed' else profiles[:1])
                self.assertEqual([c[1] for c in calls],[[320,240,500],[320,240,501],[641,359,502],[641,359,503]] if cleanup=='confirmed' else [[320,240,500]])
                result=json.loads((args.evidence/'result.json').read_text());self.assertFalse(result['cursorProofPassed']);self.assertFalse(result['passed']);self.assertFalse(result['productionCursorQualified'])
                if failed_profile is not None:self.assertTrue(any('profile failed after owned cleanup' in f['error'] for f in result['failures']))
                else:self.assertIn(profiles[0]+'-cleanup',[failure['name'] for failure in result['failures']])

    def test_missing_or_mismatched_context_rejected_before_tools_and_output(self):
        import tempfile
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);args=SimpleNamespace(evidence=root/'e',ffmpeg=root/'f',ffprobe=root/'p',scope='cursor-private')
            with patch('run.sys.platform','win32'),patch.dict(os.environ,{},clear=True),patch('run.subprocess.check_output') as tool:
                with self.assertRaises(AssertionError):run.run(args)
                tool.assert_not_called();self.assertFalse(args.evidence.exists())

class AlphaPairChecks(unittest.TestCase):
    def fixture(self,root):
        import json
        def save(path,value):path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(value))
        def pam(path,w,h,data):path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(f'P7\nWIDTH {w}\nHEIGHT {h}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n'.encode()+data)
        g=dict(window=[0,0,96,96],dwm=[0,0,96,96],clientOrigin=[0,0],pid=1,thread=2)
        clock=dict(frequency=3000000,epoch=10000000,admission=2999000,cutoff=6000000,written=30)
        base=bytearray(bytes([15,25,35,255])*96*96)
        for (cx,cy),value in zip([[8,8],[88,8],[8,88]],[40,100,160]):
            for y in range(cy-8,cy+8):
                for x in range(cx-8,cx+8):base[(y*96+x)*4:(y*96+x)*4+4]=bytes([value,value,value,255])
        for mode in ['without','with','alpha-without','alpha-with','challenge']:
            d=root/mode;alpha=mode.startswith('alpha') or mode=='challenge';source=bytes([128,64,0,128] if alpha else [255,255,255,255]);raw=base.copy()
            if mode in ['with','alpha-with']:
                raw[(48*96+48)*4:(48*96+48)*4+4]=bytes([72,45,17,255] if alpha else [255]*4)
            for i in range(2):pam(d/f'raw-{i}.pam',96,96,raw)
            rows=[]
            for i in range(9 if mode=='challenge' else 3):
                shape=dict(path=f'cursor-{i}.pam',width=1,height=1,hotspot=[0,0]);pam(d/shape['path'],1,1,source)
                if alpha:pam(d/f'native-color-{i}.pam',1,1,bytes([64,32,0,128]))
                # Hand calculated original-handle render of [128,64,0,128], including the observed black64/32 failure.
                backgrounds=[[64,32,0],[191,159,127],[81,49,17],[82,77,86]] if alpha else [[255]*3]*4
                for bg,rgb in enumerate(backgrounds):pam(d/f'raster-{i}-{bg}.pam',1,1,bytes(rgb+[255]))
                rows.append(dict(q0=3000000,q1=3000006,before=g.copy(),after=g.copy(),position=[48,48],cursorInfoPosition=[48,48],flags=1,hitRoot=3,hitPid=1,hitThread=2,shape=shape,imageError=None,consistent=True,telemetryAccepted=False))
            info=dict(rawSize=[96,96],encodedSize=[96,96],physicalAnchors=[[8,8],[88,8],[8,88]],geometry=g,observations=rows,selectedHwnd=3,withCursor=mode in ['with','alpha-with'],telemetry=[],prefix='rejectedAll',physicalLatencyClaim=False)
            save(d/'geometry.json',info)
            if mode=='challenge':
                rows[1]['flags']=0;rows[3]['hitRoot']=4;rows[6]['before']=dict(g,window=[23,17,119,113])
                save(d/'challenges.json',dict(observations=rows,telemetry=[],prefix='rejectedAll',noResumeAfterEqualPolls=True));save(d/'trace.json',dict(admission=2999000,cutoff=6000000,sourceClosed=False,sourceAvailabilityEnd=None,events=[dict(kind='geometry',qpc=6000000)]));save(d/'proof-summary.json',dict(clock=dict(cutoff=6000000,geometry_changed=True,closed=False,source_availability=None)))
        actions=['alpha','restore-cursor','alpha','hidden','restore-cursor','occlude','unocclude','move-away-back','move','move-back','minimize-restore','resize','close']
        trace,summary,first,observation,actions=resize_fixture((96,96))
        save(root/'source-actions.json',actions)
        for name,value in [('trace.json',trace),('proof-summary.json',summary),('first-write-started.json',first),('resize-observation.json',observation)]:save(root/'resize'/name,value)
        for i in range(2):pam(root/'resize'/f'raw-{i}.pam',96,96,base)
        (root/'resize/proof-with-cursor.mp4').write_bytes(b'external codec fixture')
        save(root/'visibility/visibility.json',dict(telemetry=[],prefix='rejectedAll',observations=[dict(telemetryAccepted=False,hitRoot=4)]*3))
        save(root/'closed/close-observation.json',dict(workerEndedBeforeFinish=True,manualStopRequested=False));save(root/'closed/trace.json',dict(width=96,height=96,sourceClosed=True))
        pam(root/'closed/raw-0.pam',96,96,base)
        save(root/'resource-paths.json',dict(iterations=16,before=[1,2],after=[1,2],partialAcquisitionsOwned=True,afterDibitsFailure=True,afterColorRasterFailure=True,afterColorDrawFailure=True,nativeXorRejected=True))
        save(root/'native-topology.json',dict(actualMonitors=[{}],profileAwareness='PMv2 only'))
        save(root/'proof-result.json',dict(gdiStart=1,gdiEnd=1,positionRestored=True,dpiRestored=True,showCountMutated=False))
        return clock

    def test_connected_alpha_pair_requires_real_partial_pixels_and_both_exemplars(self):
        import json,tempfile,shutil
        import cursor_check as c
        for fault in [None,'missing-pair','missing-diagnostic','missing-exemplar','wrong-alpha','doubled-candidate','wrong-placement','second-exemplar','changed-observation','opaque-alpha','unchanged-wgc','wrong-geometry']:
            with self.subTest(fault=fault),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);clock=self.fixture(root)
                if fault=='missing-pair':shutil.rmtree(root/'alpha-with')
                if fault=='missing-diagnostic':(root/'alpha-with/native-color-0.pam').unlink()
                if fault=='missing-exemplar':(root/'alpha-with/raw-1.pam').unlink()
                if fault=='unchanged-wgc':shutil.copyfile(root/'alpha-without/raw-0.pam',root/'alpha-with/raw-0.pam')
                if fault in ['wrong-alpha','doubled-candidate','opaque-alpha']:
                    p=root/'alpha-with/cursor-0.pam';data=p.read_bytes();p.write_bytes(data[:-4]+bytes({'wrong-alpha':[128,64,0,64],'doubled-candidate':[255,128,0,128],'opaque-alpha':[128,64,0,255]}[fault]))
                if fault in ['wrong-placement','second-exemplar']:
                    p=root/f'alpha-with/raw-{1 if fault=="second-exemplar" else 0}.pam';w,h,data=c.pam(p);data=bytearray(data);data[(48*w+48)*4:(48*w+48)*4+4]=bytes([15,25,35,255]);data[(48*w+49)*4:(48*w+49)*4+4]=bytes([72,45,17,255]);p.write_bytes(p.read_bytes()[:-len(data)]+data)
                if fault=='wrong-geometry':
                    p=root/'alpha-with/geometry.json';info=json.loads(p.read_text());info['geometry']['window']=[1,1,97,97];info['geometry']['dwm']=[1,1,97,97];info['geometry']['clientOrigin']=[1,1];p.write_text(json.dumps(info))
                if fault=='changed-observation':
                    p=root/'alpha-with/geometry.json';info=json.loads(p.read_text());info['observations'][1]['position']=[49,48];info['observations'][1]['cursorInfoPosition']=[49,48];p.write_text(json.dumps(info))
                # Only the external FFmpeg decode is substituted; native raster, geometry, alpha and raw overlay checks run.
                with patch('cursor_check.movie',return_value=(clock,30,{})):
                    if fault is None:
                        result=c.check(root,None,None);self.assertIn('alpha-with',result);self.assertFalse(result['productionCursorQualified'])
                    else:
                        with self.assertRaises((AssertionError,FileNotFoundError)):c.check(root,None,None)

    def test_original_raster_catches_old_doubled_candidate_even_with_matching_native_diagnostic(self):
        import tempfile
        import cursor_check as c
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);AlphaPairChecks().fixture(root);d=root/'alpha-with'
            for name,pixel in [('cursor-0.pam',[255,128,0,128]),('native-color-0.pam',[128,64,0,128])]:
                p=d/name;p.write_bytes(p.read_bytes()[:-4]+bytes(pixel))
            with self.assertRaisesRegex(AssertionError,'native raster mismatch'):
                c.raster(d,0,dict(path='cursor-0.pam',width=1,height=1,hotspot=[0,0]))

class AlphaObserverPaths(unittest.TestCase):
    def test_observer_holds_exact_handles_for_both_alpha_proofs_only_at_named_paths(self):
        import json,sys,tempfile
        driver_code="""import json,os,sys,time
from pathlib import Path
root=Path(sys.argv[1]);gates=[]
for i,name in enumerate(['without','with','alpha-without','alpha-with','challenge','resize','closed','alpha-probe','unexpected','nested/alpha-with']):
    d=root/'session-0'/name;d.mkdir(parents=True,exist_ok=True)
    (d/'owned.json').write_text(json.dumps(dict(parent=os.getpid(),worker=100+i*2,encoder=101+i*2,qpc=123+i)))
    if i<7:gates.append(d/'handles-open')
while not all(p.exists() for p in gates):time.sleep(.005)
"""
        handles=[]
        class Handle:
            def __init__(self,pid):self.pid=pid;self.closed=False;handles.append(self)
            def signaled(self):return True
            def close(self):self.closed=True
            def kill(self):raise AssertionError('successful observer must not kill')
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);driver=root/'driver.py';driver.write_text(driver_code)
            with patch('run.ExactHandle',Handle),patch('run.native_qpc',return_value=123):
                run.execute([sys.executable,driver,root/'case'],root,'cursor-borderless-even',3,observe=root/'case')
            self.assertEqual([h.pid for h in handles],list(range(100,114)));self.assertTrue(all(h.closed for h in handles))
            receipt=json.loads((root/'cursor-borderless-even-command.json').read_text())
            self.assertEqual(len(receipt['ownership']),7);self.assertIsNone(receipt['error'])
            self.assertTrue(all(r['signaled']==[True,True] and r['fallback'] is False for r in receipt['ownership']))
            expected={f'session-0/{name}/owned.json' for name in PRIVATE_PROOFS}
            self.assertEqual({row['path'] for row in receipt['cleanup']['owned']},expected)
            for row in receipt['cleanup']['owned']:
                original=json.loads((root/'case'/row['path']).read_text())
                self.assertTrue((root/'case'/row['path']).with_name('handles-open').is_file())
                self.assertTrue(all(row[key]==original[key] for key in original))
                self.assertEqual(row['signaled'],[True,True])
            for name in ['alpha-probe','unexpected','nested/alpha-with']:self.assertFalse((root/'case/session-0'/name/'handles-open').exists())


class PendingFileBounds(unittest.TestCase):
    def test_atomic_pending_rename_during_bound_scan_is_not_a_native_failure(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);case=root/'cursor-borderless-even/session-0';case.mkdir(parents=True);pending=case/'source-request.pending';pending.write_text('{}');published=pending.with_suffix('.json')
            original=Path.is_file
            def is_file(path,*args,**kwargs):
                result=original(path,*args,**kwargs)
                if path==pending and result:pending.rename(published)
                return result
            # Real rename after is_file's successful check reproduces the observer's two-read race.
            with patch.object(Path,'is_file',is_file):run.bound(root)
            self.assertEqual(published.read_text(),'{}');self.assertFalse(pending.exists())
            oversized=root/'trace.json'
            with oversized.open('wb') as out:out.truncate(4*1024*1024+1)
            with self.assertRaisesRegex(AssertionError,'4MiB trace cap'):run.bound(root)

    def test_disappearing_permanent_evidence_still_fails_bound_scan(self):
        import tempfile
        for name in ['owned.json','unknown.pending']:
            with self.subTest(name=name),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);receipt=root/name;receipt.write_text('{}');original=Path.is_file
                def is_file(path,*args,**kwargs):
                    result=original(path,*args,**kwargs)
                    if path==receipt and result:receipt.unlink()
                    return result
                with patch.object(Path,'is_file',is_file),self.assertRaises(FileNotFoundError):run.bound(root)

    def test_pending_symlink_does_not_hide_disappearing_target(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);case=root/'cursor-borderless-even/session-0';case.mkdir(parents=True);target=root/'target.json';target.write_text('{}');pending=case/'source-request.pending';pending.symlink_to(target);original=Path.is_file
            def is_file(path,*args,**kwargs):
                result=original(path,*args,**kwargs)
                if path==pending and result:target.unlink()
                return result
            with patch.object(Path,'is_file',is_file),self.assertRaises(FileNotFoundError):run.bound(root)

    def test_execute_retains_missing_filename_and_bounded_traceback(self):
        import json,sys,tempfile
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);receipt=root/'owned.json';receipt.write_text('{}');original=Path.is_file
            def is_file(path,*args,**kwargs):
                result=original(path,*args,**kwargs)
                if path==receipt and result:receipt.unlink()
                return result
            with patch.object(Path,'is_file',is_file),self.assertRaises(RuntimeError):
                run.execute([sys.executable,'-c','import time;time.sleep(5)'],root,'bound-failure',3)
            failure=json.loads((root/'bound-failure-command.json').read_text())
            self.assertIn('FileNotFoundError',failure['error']);self.assertEqual(failure.get('errorFilename'),str(receipt))
            self.assertIn('bound',failure.get('errorTraceback',''));self.assertIn('owned.json',failure.get('errorTraceback',''))
            self.assertLessEqual(len(failure['errorTraceback']),8192);self.assertIsNotNone(failure['exitCode'])

class Codec420Crop(unittest.TestCase):
    def test_full_coded_planes_keep_odd_crop_chroma_phase(self):
        import cursor_check as c
        check=c.codec_crop
        # 6x4 coded frame:24 Y samples, six distinct U cells, six distinct V cells.
        expected=bytes([64]*24+[16,100,200,32,110,210]+[220,120,30,230,130,40]);actual=bytearray(expected);actual[0]=255
        metrics=check(actual,expected,6,4,[1,1,5,3]);self.assertEqual([metrics[p]['samples'] for p in ['Y','U','V']],[8,6,6]);self.assertTrue(all(metrics[p]['max']==0 for p in ['Y','U','V']))
        for index in [7,24,35]:
            with self.subTest(index=index):
                actual=bytearray(expected);actual[index]+=49
                with self.assertRaisesRegex(AssertionError,'codec-tolerant 420'):check(actual,expected,6,4,[1,1,5,3])

    def test_existing_numeric_codec_limits_apply_to_each_plane(self):
        import cursor_check as c
        check=c.codec_crop
        expected=bytes([64]*36)
        for start,end in [(0,24),(24,30),(30,36)]:
            actual=bytearray(expected);actual[start+7 if start==0 else start]=112
            check(actual,expected,6,4,[1,1,5,3]) # maximum48 with mean below12 passes.
            actual=bytearray(expected);actual[start:end]=bytes([77])*(end-start)
            with self.subTest(plane=(start,end)),self.assertRaisesRegex(AssertionError,'codec-tolerant 420'):check(actual,expected,6,4,[1,1,5,3]) # mean13 fails, despite max13.

    def test_planar_size_even_dimensions_and_crop_are_required(self):
        import cursor_check as c
        check=c.codec_crop
        for actual,expected,w,h,crop in [(bytes(35),bytes(36),6,4,[1,1,5,3]),(bytes(36),bytes(35),6,4,[1,1,5,3]),(bytes(36),bytes(36),5,4,[1,1,4,3]),(bytes(36),bytes(36),6,4,[2,1,2,3]),(bytes(36),bytes(36),6,4,[-1,1,5,3]),(bytes(36),bytes(36),6,4,[1,1,7,3])]:
            with self.subTest(w=w,h=h,crop=crop),self.assertRaises(AssertionError):check(actual,expected,w,h,crop)

class OwnedAcquisitionScope(unittest.TestCase):
    def test_portable_source_guard_keeps_owned_snapshot_between_real_qpc_queries(self):
        # Static ordering guard only: this is not a Windows timing/resource execution test.
        repo=Path(__file__).resolve().parents[2]
        source=(repo/'FocusFlow/src-tauri/src/recorder/windows_qualification/cursor.rs').read_text()
        observe=source.split('fn observe(',1)[1].split('// Fixture command history',1)[0]
        acquisition=observe.split('let q0=',1)[1].split('let q1=',1)[0]
        self.assertIn('CopyIcon(',acquisition,'owned native snapshot must be acquired within q0/q1')
        self.assertEqual(observe.count('CopyIcon('),1)
        self.assertEqual(acquisition.count('GetCursorInfo('),2);self.assertEqual(acquisition.count('GetPhysicalCursorPos('),2);self.assertEqual(acquisition.count('geometry(hwnd)'),2)
        self.assertLess(acquisition.index('GetCursorInfo('),acquisition.index('CopyIcon('));self.assertLess(acquisition.index('CopyIcon('),acquisition.rindex('GetCursorInfo('))
        for deferred in ['copied_shape(','owned_shape(','GetIconInfo(','bitmap(','DrawIconEx(','native_raster(','pam(','fs::']:
            self.assertNotIn(deferred,acquisition,'validation and filesystem work must be deferred until after q1')
        self.assertNotIn('copied_shape(',observe,'observer must not acquire a second copy after q1')
        self.assertLess(observe.index('let q1='),observe.index('owned_shape('))
        owned=source.split('fn owned_shape(',1)[1].split('fn pam(',1)[0]
        self.assertNotIn('CopyIcon(',owned,'deferred extraction consumes the already-owned snapshot')
        self.assertIn('native_raster(HICON(original.0)',owned,'native oracle must retain the original borrowed handle')
        wrapper=source.split('fn copied_shape(',1)[1].split('fn owned_shape(',1)[0]
        self.assertIn('fail_stage==1',wrapper);self.assertIn('owned_shape(',wrapper)

class StartupChecks(unittest.TestCase):
    def fixture(self,root):
        import json
        w,h=322,272
        startup=dict(allocation=[336,279],frozen=[322,272],itemCreations=1,captureStarts=1,metadataCallbacks=5,mismatches=1,
                     startQpc=2990000,freezeQpc=2991000,probeQpc=2992000,preparedQpc=2993000,gate2Qpc=2994000,encoderQpc=2995000,gate4Qpc=2996000,parkEnterQpc=2997000,parkLeaveQpc=2997100,supervisorQpc=2998000,writerQpc=2999000,armQpc=3000000,
                     pixelCopies=2,preArmPixelCopies=0,preAdmissionPixelCopies=0,sourcePublications=2,preArmSourcePublications=0,proofWrites=2,preArmProofWrites=0,cleanupConsumptions=1)
        source=dict(kind='source',order=0,seq=0,native=10000000,qpc=3000010,contentWidth=w,contentHeight=h,textureWidth=w,textureHeight=h)
        trace=dict(startup=startup,frequency=3000000,admission=3000000,epoch=10000000,cutoff=3100000,selected=1,written=1,width=w,height=h,overflow=False,events=[source,dict(source,order=1,seq=1,native=10000001,qpc=3000020),dict(kind='decision',order=2,ordinal=0,seq=0,qpc=3100000)],writes=[dict(ordinal=0,seq=0,bytes=w*h*4,ok=True)])
        clock={k:trace[k] for k in ['frequency','admission','epoch','cutoff','selected','written']}
        (root/'proof-summary.json').write_text(json.dumps(dict(privateProof=True,neverImport=True,workerExit=0,encoderSignaled=True,clock=clock)))
        (root/'proof-without-cursor.mp4').write_bytes(b'fixture; external codec substituted')
        pixels=bytes([40,40,40,255])*(w*h)
        for i in range(2):(root/f'raw-{i}.pam').write_bytes(f'P7\nWIDTH {w}\nHEIGHT {h}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n'.encode()+pixels)
        info=dict(rawSize=[w,h],encodedSize=[w,h],withCursor=False)
        meta=dict(streams=[dict(width=w,height=h,codec_name='h264',pix_fmt='yuv420p',time_base='1/30')],packets=[dict(pts=0,dts=0,duration=1)])
        return trace,info,meta

    def movie(self,root,trace,info,meta):
        import io,json
        import cursor_check as c
        (root/'trace.json').write_text(json.dumps(trace))
        class Decoder:
            def __init__(self,*unused,**kwargs):self.stdout=io.BytesIO(bytes([40]*3)*(meta['streams'][0]['width']*meta['streams'][0]['height']))
            def poll(self):return 0
            def wait(self,**kwargs):return 0
            def kill(self):raise AssertionError('fixture decoder should have finished')
        def output(argv,**unused):return 'fixture ffmpeg\n' if '-version' in argv else json.dumps(meta)
        with patch('cursor_check.subprocess.check_output',side_effect=output),patch('cursor_check.subprocess.Popen',Decoder):
            return c.movie(root,info,'ffmpeg','ffprobe',[[8,8]])

    def test_initial_allocation_is_not_the_frozen_media_size(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);trace,info,meta=self.fixture(root)
            self.assertEqual(self.movie(root,trace,info,meta)[1],1)

    def test_odd_frozen_media_retains_right_bottom_coded_padding(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);trace,info,meta=self.fixture(root)
            trace.update(width=321,height=271);trace['startup']['frozen']=[321,271];info['rawSize']=[321,271]
            for event in trace['events']:
                if event['kind']=='source':event.update(contentWidth=321,textureWidth=321,contentHeight=271,textureHeight=271)
            self.assertEqual(self.movie(root,trace,info,meta)[1],1)

    def test_startup_receipt_and_actual_order_are_required(self):
        import tempfile
        faults=['missing','media','freeze-after-arm','pre-arm-copy','pre-admission-copy','second-item','second-start','cleanup-twice','arm-disagrees','probe-before-freeze','writer-before-park','source-pre-arm','source-native-pre-arm','source-publication-pre-arm','epoch','counter-disagrees','bool-counter','missing-clock','raw-exemplar','decoded-dimensions','decoded-stream-only']
        for fault in faults:
            with self.subTest(fault=fault),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);trace,info,meta=self.fixture(root);s=trace['startup']
                if fault=='missing':del trace['startup']
                if fault=='media':s['frozen']=[336,279]
                if fault=='freeze-after-arm':s['freezeQpc']=3000001
                if fault=='pre-arm-copy':s['preArmPixelCopies']=1
                if fault=='pre-admission-copy':s['preAdmissionPixelCopies']=1
                if fault=='second-item':s['itemCreations']=2
                if fault=='second-start':s['captureStarts']=2
                if fault=='cleanup-twice':s['cleanupConsumptions']=2
                if fault=='arm-disagrees':s['armQpc']=2999999
                if fault=='probe-before-freeze':s['probeQpc']=2990999
                if fault=='writer-before-park':s['writerQpc']=2996999
                if fault in {'source-pre-arm','source-native-pre-arm'}:
                    early=dict(trace['events'][0],native=9999999,qpc=2999999 if fault=='source-pre-arm' else 3000000)
                    for e in trace['events']:
                        e['order']+=1;e['seq']+=1
                    trace['events'].insert(0,early);trace['writes'][0]['seq']=1;s['sourcePublications']=3;s['pixelCopies']=3;s['metadataCallbacks']=6
                if fault=='source-publication-pre-arm':trace['events'][0]['qpc']=2999999
                if fault=='epoch':trace['epoch']=9999999
                if fault=='counter-disagrees':s['sourcePublications']=1
                if fault=='bool-counter':s['itemCreations']=True
                if fault=='missing-clock':del s['parkLeaveQpc']
                if fault=='raw-exemplar':
                    p=root/'raw-1.pam';p.write_bytes(p.read_bytes().replace(b'WIDTH 322\n',b'WIDTH 320\n',1)[:-2*272*4])
                if fault=='decoded-dimensions':
                    info['encodedSize']=[336,280];meta['streams'][0].update(width=336,height=280)
                if fault=='decoded-stream-only':meta['streams'][0].update(width=336,height=280)
                with self.assertRaises((AssertionError,KeyError)):self.movie(root,trace,info,meta)

    def test_unit_origin_requires_real_noncollinear_markers_even_with_size_delta(self):
        import cursor_check as c
        raw=[[8,8],[300,8],[8,250]]
        for origin in [[100,50],[-100,-50],[0,-50]]:
            physical=[[x+origin[0],y+origin[1]] for x,y in raw]
            self.assertEqual(c.unit_origin(raw,physical),origin)
            for bad in [raw[:2],[[8,8],[9,9],[10,10]],[[16,16],[600,16],[16,500]],[[9,8],[300,8],[8,250]]]:
                with self.assertRaises(AssertionError):c.unit_origin(bad,physical)
        # Allocation/media differences are not evidence of a translated pixel origin.
        with self.assertRaises(AssertionError):c.unit_origin([],[[14,7]]*3)

    def test_missing_ambiguous_and_padding_fiducials_fail_with_odd_padding_positive(self):
        import cursor_check as c
        w,h=66,66;raw=bytearray(w*h*4)
        def marker(p,x,y,value):
            for yy in range(y,y+16):
                for xx in range(x,x+16):p[(yy*w+xx)*4:(yy*w+xx)*4+4]=bytes([value]*3+[255])
        for x,y,v in [(0,0,40),(32,0,100),(0,32,160)]:marker(raw,x,y,v)
        self.assertEqual(c.anchors(w,h,raw,65,65),[[8,8],[40,8],[8,40]])
        for fault in ['missing','ambiguous','padding']:
            bad=raw.copy()
            if fault=='missing':marker(bad,0,32,0)
            if fault=='ambiguous':marker(bad,32,32,40)
            if fault=='padding':marker(bad,50,32,160);marker(bad,0,32,0)
            with self.subTest(fault=fault):
                if fault=='padding':
                    # The 16px marker extends into the right pad of a 65px raw frame.
                    with self.assertRaises(AssertionError):c.anchors(w,h,bad,65,65)
                else:
                    with self.assertRaises(AssertionError):c.anchors(w,h,bad)


def resize_fixture(raw=(322,272)):
    # Two complete 30Hz cells on a 3MHz clock, with a cutoff during SetWindowPos.
    w,h=raw
    stages=['startQpc','freezeQpc','probeQpc','preparedQpc','gate2Qpc','encoderQpc','gate4Qpc','parkEnterQpc','parkLeaveQpc','supervisorQpc','writerQpc','armQpc']
    startup=dict(zip(stages,range(2989000,3000001,1000)))
    startup.update(allocation=[w+14,h+7],frozen=[w,h],itemCreations=1,captureStarts=1,cleanupConsumptions=1,metadataCallbacks=5,mismatches=1,pixelCopies=2,preArmPixelCopies=0,preAdmissionPixelCopies=0,sourcePublications=2,preArmSourcePublications=0,proofWrites=2,preArmProofWrites=0)
    events=[dict(kind='source',order=i,seq=i,native=10000000+i,qpc=3000010+i*10,contentWidth=w,contentHeight=h,textureWidth=w,textureHeight=h) for i in range(2)]
    events += [dict(kind='decision',order=2+i,ordinal=i,seq=i,qpc=3100000+i*100000) for i in range(2)]
    events.append(dict(kind='geometry',order=4,qpc=3200000,contentWidth=w+2,contentHeight=h+2,textureWidth=w,textureHeight=h))
    trace=dict(startup=startup,frequency=3000000,admission=3000000,epoch=10000000,cutoff=3200000,selected=2,written=2,width=w,height=h,overflow=False,sourceClosed=False,sourceAvailabilityEnd=None,events=events,writes=[dict(ordinal=i,seq=i,bytes=(w+w%2)*(h+h%2)*4,ok=True) for i in range(2)])
    summary=dict(privateProof=True,neverImport=True,workerExit=0,encoderSignaled=True,clock={k:trace[k] for k in ['frequency','admission','epoch','cutoff','selected','written']})
    summary['clock'].update(geometry_changed=True,closed=False,source_availability=None)
    first=dict(qpc=3100001,seq=0)
    observation=dict(actionSequence=12,liveCheckBeforeQpc=3100010,liveCheckAfterQpc=3100020,workerAliveBeforeResize=True,workerEndedBeforeFinish=True,endedObservedQpc=3200100,manualStopRequested=False)
    names=['alpha','restore-cursor','alpha','hidden','restore-cursor','occlude','unocclude','move-away-back','move','move-back','minimize-restore','resize','close']
    records=[dict(action=a,sequence=i+1,operationBeforeQpc=100000+i*10000,operationAfterQpc=100001+i*10000,win32Succeeded=True) for i,a in enumerate(names)]
    records[11].update(commandReadBeforeQpc=3199950,commandReadAfterQpc=3199960,operationBeforeQpc=3199980,operationAfterQpc=3200010,windowRectBefore=[-40,40,w-40,h+40],windowRectAfter=[-40,40,w-38,h+42])
    records[12].update(commandReadBeforeQpc=3300000,commandReadAfterQpc=3300001,operationBeforeQpc=3300002,operationAfterQpc=3300003)
    return trace,summary,first,observation,dict(qpcFrequency=3000000,overflow=False,records=records)

class ResizeAttributionChecks(unittest.TestCase):
    def checker(self,*args):
        import cursor_check as c
        self.assertTrue(callable(getattr(c,'resize_attribution',None)),'shared checker lacks explicit resize attribution')
        return c.resize_attribution(*args)

    def test_non10mhz_borderless_decorated_and_during_call_cutoffs(self):
        import analyze
        for raw in [(320,240),(641,359),(322,272),(643,391)]:
            for new_texture in [False,True]:
                for during in [False,True]:
                    with self.subTest(raw=raw,new_texture=new_texture,during=during):
                        t,s,f,o,a=resize_fixture(raw)
                        if new_texture:t['events'][-1].update(textureWidth=raw[0]+2,textureHeight=raw[1]+2)
                        if not during:a['records'][11]['operationAfterQpc']=3199990
                        self.assertEqual(len(analyze.expected(t)),2)
                        result=self.checker(t,s,f,o,a)
                        self.assertEqual(result['frozenSize'],list(raw));self.assertEqual(result['actionSequence'],12)
                        self.assertEqual(result['oldTextureNewContentObserved'],not new_texture)

    def test_rejects_earlier_unrelated_manual_and_wrong_clock_resize_claims(self):
        def cutoff_at(t,s,value):
            t['cutoff']=s['clock']['cutoff']=t['events'][-1]['qpc']=value
        faults={
            'missing-live':lambda t,s,f,o,a:o.pop('workerAliveBeforeResize'),
            'false-live':lambda t,s,f,o,a:o.update(workerAliveBeforeResize=False),
            'manual-stop':lambda t,s,f,o,a:o.update(manualStopRequested=True),
            'not-ended':lambda t,s,f,o,a:o.update(workerEndedBeforeFinish=False),
            'end-before-action':lambda t,s,f,o,a:o.update(endedObservedQpc=3199970),
            'first-after-action':lambda t,s,f,o,a:f.update(qpc=3199990),
            'first-before-publication':lambda t,s,f,o,a:f.update(qpc=3000005),
            'wrong-written-seq':lambda t,s,f,o,a:t['writes'][0].update(seq=1),
            'wrong-decision-seq':lambda t,s,f,o,a:t['events'][2].update(seq=1),
            'wrong-first-seq':lambda t,s,f,o,a:f.update(seq=1),
            'late-admission':lambda t,s,f,o,a:t.update(admission=3199990),
            'earlier-cutoff':lambda t,s,f,o,a:cutoff_at(t,s,3199970),
            'same-tick-cutoff':lambda t,s,f,o,a:cutoff_at(t,s,3199980),
            'second-event':lambda t,s,f,o,a:t['events'].append(dict(t['events'][-1])),
            'missing-event':lambda t,s,f,o,a:t['events'].pop(),
            'wrong-cutoff':lambda t,s,f,o,a:t.update(cutoff=3200001),
            'failed-action':lambda t,s,f,o,a:a['records'][11].update(win32Succeeded=False),
            'missing-action':lambda t,s,f,o,a:a['records'].pop(11),
            'duplicate-action':lambda t,s,f,o,a:a['records'].append(dict(a['records'][11])),
            'wrong-sequence':lambda t,s,f,o,a:a['records'][11].update(sequence=11),
            'wrong-observed-sequence':lambda t,s,f,o,a:o.update(actionSequence=13),
            'wrong-frequency':lambda t,s,f,o,a:a.update(qpcFrequency=10000000),
            'bool-frequency':lambda t,s,f,o,a:a.update(qpcFrequency=True),
            'overflow':lambda t,s,f,o,a:a.update(overflow=True),
            'restore-overlaps-startup':lambda t,s,f,o,a:a['records'][10].update(operationAfterQpc=3000001),
            'close-before-end':lambda t,s,f,o,a:a['records'][12].update(commandReadBeforeQpc=3200099),
            'unchanged-window':lambda t,s,f,o,a:a['records'][11].update(windowRectAfter=a['records'][11]['windowRectBefore']),
            'moved-window':lambda t,s,f,o,a:a['records'][11].update(windowRectAfter=[-39,40,285,314]),
            'bool-rect':lambda t,s,f,o,a:a['records'][11]['windowRectAfter'].__setitem__(0,True),
            'unchanged-content':lambda t,s,f,o,a:t['events'][-1].update(contentWidth=322,contentHeight=272),
            'unrelated-plus7':lambda t,s,f,o,a:t['events'][-1].update(contentHeight=279),
            'client-grid':lambda t,s,f,o,a:t['events'][-1].update(contentWidth=322,contentHeight=242),
            'late-publication':lambda t,s,f,o,a:t['events'][0].update(qpc=3200000),
            'changed-source':lambda t,s,f,o,a:t['events'][1].update(textureHeight=279),
            'wrong-summary':lambda t,s,f,o,a:s['clock'].update(geometry_changed=False),
            'closed':lambda t,s,f,o,a:t.update(sourceClosed=True),
            'unavailable':lambda t,s,f,o,a:s['clock'].update(source_availability='unavailable'),
        }
        for name,row,key in [('admission',0,'admission'),('cutoff',0,'cutoff'),('frequency',0,'frequency'),('first-write',2,'qpc')]:
            for value in [None,True,-1]:faults[f'{name}-{value}']=lambda *args,row=row,key=key,value=value:args[row].update({key:value})
        for kind,index in [('source',0),('geometry',-1)]:
            for value in [None,True,-1]:faults[f'{kind}-qpc-{value}']=lambda t,s,f,o,a,index=index,value=value:t['events'][index].update(qpc=value)
        for field in ['liveCheckBeforeQpc','liveCheckAfterQpc','endedObservedQpc']:
            for value in [None,True,-1]:faults[f'{field}-{value}']=lambda t,s,f,o,a,field=field,value=value:o.update({field:value})
        for field in ['commandReadBeforeQpc','commandReadAfterQpc','operationBeforeQpc','operationAfterQpc']:
            for value in [None,True,-1]:faults[f'{field}-{value}']=lambda t,s,f,o,a,field=field,value=value:a['records'][11].update({field:value})
        for name,mutate in faults.items():
            with self.subTest(fault=name):
                args=resize_fixture();mutate(*args)
                with self.assertRaises((AssertionError,KeyError),msg=name):self.checker(*args)

    def test_connected_path_requires_resize_receipt_and_raw_exemplars(self):
        import cursor_check as c
        import tempfile,shutil,io,json
        for fault in [None,'directory','receipt','first-write','raw-0','raw-1','earlier-event']:
            with self.subTest(fault=fault),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);clock=AlphaPairChecks().fixture(root)
                if fault=='directory':shutil.rmtree(root/'resize',ignore_errors=True)
                elif fault in ['receipt','first-write','raw-0','raw-1']:
                    name='resize-observation.json' if fault=='receipt' else 'first-write-started.json' if fault=='first-write' else fault+'.pam';(root/'resize'/name).unlink(missing_ok=True)
                elif fault=='earlier-event':
                    import json
                    p=root/'resize/trace.json';t=json.loads(p.read_text());t['events'][-1]['qpc']=3199970;p.write_text(json.dumps(t))
                movie=c.movie
                class Decoder:
                    def __init__(self,*unused,**kwargs):
                        _,_,rgba=c.pam(root/'resize/raw-0.pam');rgb=b''.join(rgba[i:i+3] for i in range(0,len(rgba),4));self.stdout=io.BytesIO(rgb*2)
                    def poll(self):return 0
                    def wait(self,**kwargs):return 0
                    def kill(self):raise AssertionError('fixture decoder should have finished')
                def output(argv,**unused):
                    if '-version' in argv:return 'fixture decoder\n'
                    return json.dumps(dict(streams=[dict(width=96,height=96,codec_name='h264',pix_fmt='yuv420p',time_base='1/30')],packets=[dict(pts=i,dts=i,duration=1) for i in range(2)]))
                def media(directory,*args,**kwargs):return movie(directory,*args,**kwargs) if directory.name=='resize' else (clock,30,{})
                with patch('cursor_check.movie',side_effect=media),patch('cursor_check.subprocess.check_output',side_effect=output),patch('cursor_check.subprocess.Popen',Decoder):
                    if fault is None:self.assertIn('explicitResize',c.check(root,None,None))
                    else:
                        with self.assertRaises((AssertionError,KeyError,FileNotFoundError)):c.check(root,None,None)

    def test_challenge_termination_does_not_claim_explicit_resize_or_postcutoff_live_interval(self):
        import cursor_check as c
        import tempfile,json
        for cause in ['geometry','manual','contradictory','closed','unavailable']:
            with self.subTest(cause=cause),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);clock=AlphaPairChecks().fixture(root)
                p=root/'challenge/trace.json';t=json.loads(p.read_text());t['cutoff']=3000003
                if cause=='manual':t['events']=[]
                if cause=='closed':t['sourceClosed']=True
                if cause=='unavailable':t['sourceAvailabilityEnd']='unavailable'
                if t['events']:t['events'][0]['qpc']=t['cutoff']
                p.write_text(json.dumps(t));p=root/'challenge/proof-summary.json';s=json.loads(p.read_text());s['clock'].update(cutoff=t['cutoff'],geometry_changed=cause!='manual')
                if cause=='contradictory':s['clock']['geometry_changed']=False
                p.write_text(json.dumps(s))
                with patch('cursor_check.movie',return_value=(clock,30,{})):
                    if cause in ['geometry','manual']:
                        result=c.check(root,None,None)['challenges'];self.assertIn('terminationCause',result);self.assertEqual(result['terminationCause'],'geometryChanged' if cause=='geometry' else 'manualFinish');self.assertEqual(result['observationsWhollyBeforeCutoff'],[]);self.assertEqual(result['observationsOutsideRecordingInterval'],list(range(9)));self.assertNotIn('explicitResize',result)
                    else:
                        with self.assertRaises(AssertionError):c.check(root,None,None)


class ResizeActionBounds(unittest.TestCase):
    def test_actual_rectangle_receipt_fits_existing_source_action_cap_at_scalar_limits(self):
        import json
        actions=resize_fixture()[4];actions['qpcFrequency']=2**63-1
        for row in actions['records']:
            for key in list(row):
                if key.endswith('Qpc'):row[key]=2**63-1
        actions['records'][11].update(windowRectBefore=[-2**31,-2**31,2**31-3,2**31-3],windowRectAfter=[-2**31,-2**31,2**31-1,2**31-1],errorHresult=None)
        actions['records'][12]['errorHresult']=None
        self.assertEqual(len(actions['records']),13)
        self.assertLessEqual(len(json.dumps(actions,separators=(',',':')).encode()),4096)


PRIVATE_PROOFS=['without','with','alpha-without','alpha-with','challenge','resize','closed']

def private_owner_fixture(root,receipt):
    case=root/'session-0';case.mkdir(parents=True);owned=[];observations=[]
    for index,name in enumerate(PRIVATE_PROOFS):
        d=case/name;d.mkdir();value=dict(parent=10,worker=100+index*2,encoder=101+index*2,qpc=123+index);run.write(d/'owned.json',value)
        owned.append(dict(value,path=f'session-0/{name}/owned.json',signaled=[True,True]))
        observations.append(dict(value,signaled=[True,True],fallback=False))
    source=dict(pid=40,observed=True,signaled=True,exitCode=0);run.write(case/'source-cleanup.json',source)
    data=dict(exitCode=0,error=None,ownership=observations,cleanup=dict(parent=dict(pid=10,signaled=True,exitCode=0),owned=owned,sources=[source]));run.write(receipt,data);return data

class ResizeOwnership(unittest.TestCase):
    def test_seven_named_owner_observations_are_required_only_for_complete_success(self):
        import tempfile,inspect,shutil
        self.assertIn('complete',inspect.signature(run.require_private_cleanup).parameters,'driver lacks successful seven-worker completion gate')
        for fault in [None,'missing-resize','mismatched-tuple','unsignaled-resize','missing-seventh-observation','unknown-owner','duplicate-observation','fallback','aliased-worker']:
            with self.subTest(fault=fault),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp)/'case';receipt=Path(tmp)/'command.json';data=private_owner_fixture(root,receipt)
                if fault=='missing-resize':
                    shutil.rmtree(root/'session-0/resize');data['cleanup']['owned'].pop(5);data['ownership'].pop(5)
                if fault=='mismatched-tuple':data['ownership'][5]['worker']=999
                if fault=='unsignaled-resize':data['ownership'][5]['signaled'][0]=False
                if fault=='missing-seventh-observation':data['ownership'].pop(5)
                if fault=='unknown-owner':
                    p=root/'session-0/unknown/owned.json';p.parent.mkdir();run.write(p,dict(parent=10,worker=999,encoder=998,qpc=0))
                if fault=='duplicate-observation':data['ownership'][5]=dict(data['ownership'][4])
                if fault=='fallback':data['ownership'][5]['fallback']=True
                if fault=='aliased-worker':
                    data['cleanup']['owned'][5]['worker']=100;data['ownership'][5]['worker']=100;run.write(root/'session-0/resize/owned.json',{key:data['cleanup']['owned'][5][key] for key in ['parent','worker','encoder','qpc']})
                run.write(receipt,data)
                if fault is None:run.require_private_cleanup(root,receipt,complete=True)
                else:
                    with self.assertRaises(AssertionError,msg=fault):run.require_private_cleanup(root,receipt,complete=True)
                if fault=='missing-resize':run.require_private_cleanup(root,receipt) # A cleaned failed attempt can allow collection, never success.

    def test_resize_package_retains_symlink_file_and_aggregate_caps(self):
        import tempfile,json
        from package_evidence import package
        for fault in ['symlink','trace-cap','pam-cap','movie-cap','aggregate-cap']:
            with self.subTest(fault=fault),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);ev=root/'e';d=ev/'cursor-borderless-even/session-0/resize';d.mkdir(parents=True)
                if fault=='symlink':
                    target=root/'target';target.write_text('{}');(d/'resize-observation.json').symlink_to(target)
                elif fault=='aggregate-cap':
                    # Sparse generated files exercise the real 255MiB accounting without filling the disk.
                    for mode in ['without','with','alpha-without','resize']:
                        p=d.parent/mode/'proof-with-cursor.mp4';p.parent.mkdir(exist_ok=True)
                        with p.open('wb') as f:f.truncate(64*1024*1024)
                else:
                    name,limit={'trace-cap':('trace.json',4*1024*1024),'pam-cap':('raw-1.pam',9*1024*1024),'movie-cap':('proof-with-cursor.mp4',64*1024*1024)}[fault]
                    with (d/name).open('wb') as f:f.truncate(limit+1)
                with self.assertRaises(RuntimeError):package(ev,root/'absent',root/'out')
                manifest=json.loads((root/'out/upload-manifest.json').read_text());self.assertTrue(manifest['capFailure']);self.assertTrue(manifest['omitted']);self.assertLessEqual(manifest['bytes'],255*1024*1024)
