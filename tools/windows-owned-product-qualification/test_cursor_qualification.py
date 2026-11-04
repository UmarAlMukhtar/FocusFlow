"""Private lane routing and independent mathematical/resource trust boundaries; no native calls."""
import os
from pathlib import Path
import unittest
from unittest.mock import patch
import run

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
            hashes=package(ev,root/'absent',root/'qualification-upload')
            expected={f'cases/cursor-borderless-even/session-0/with/{n}' for n in ['raw-0.pam','cursor-0.pam','raster-0-0.pam','native-color-0.pam','native-color-11.pam','proof-with-cursor.mp4','geometry.json']}|{'cases/cursor-borderless-even/session-0/source-command-13.json','cases/cursor-borderless-even/session-0/alpha-with/native-color-0.pam'}
            expected|={f'cases/cursor-borderless-even/session-0/alpha-probe/{n}' for n in ['cursor-10.pam','native-color-10.pam']+[f'raster-10-{i}.pam' for i in range(4)]}
            self.assertEqual(set(hashes),expected)
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
    def test_private_scope_builds_one_harness_and_stops_on_first_required_failure(self):
        import json,tempfile
        from types import SimpleNamespace
        for fail in [False,True]:
            with self.subTest(fail=fail),tempfile.TemporaryDirectory() as tmp:
                repo=Path(__file__).resolve().parents[2];root=Path(tmp);args=SimpleNamespace(evidence=root/'evidence',ffmpeg=root/'f',ffprobe=root/'p',scope='cursor-private')
                metadata={'packages':[{'name':'windows-capture','source':None,'manifest_path':str(repo/'FocusFlow/vendor/windows-capture/Cargo.toml')}]};calls=[];builds=[]
                def execute(argv,evidence,name,*unused,**kwargs):
                    calls.append(name)
                    if name in {'cursor-portable','cursor-renderer'}:
                        log=evidence/(name+'.log');log.write_text('test result: ok. '+('6' if name=='cursor-portable' else '1')+' passed; 0 failed; 0 ignored;');return log
                    self.assertTrue(name.startswith('cursor-'));self.assertEqual(argv[2],'cursor-private')
                    if fail:raise RuntimeError('required first native profile failed')
                def build(repo,evidence,manifest,name,test):builds.append((name,test));return root/'harness'
                with patch('run.sys.platform','win32'),patch.dict(os.environ,{'GITHUB_REF':'refs/heads/codex-windows-cursor-qualification'}),patch('run.require_cursor_disposable_context',return_value={'sha':'a'*40}),patch('run.os.chdir'),patch('run.source_manifest',return_value={}),patch('run.sha',side_effect=lambda p:run.FFMPEG_SHA if p==args.ffmpeg else run.FFPROBE_SHA if p==args.ffprobe else 'fixture-hash'),patch('run.subprocess.check_output',side_effect=lambda argv,**kw:json.dumps(metadata) if argv[0]=='cargo' else 'a'*40),patch('run.execute',side_effect=execute),patch('run.build_exe',side_effect=build),patch('cursor_check.check') as checker:
                    if fail:
                        with self.assertRaises(RuntimeError):run.run(args)
                    else:run.run(args)
                self.assertEqual(builds,[('windows_owned_qualification',True)])
                self.assertEqual(len(calls),3 if fail else 6);self.assertEqual(checker.call_count,0 if fail else 4)
                result=json.loads((args.evidence/'result.json').read_text());self.assertFalse(result['passed']);self.assertFalse(result['productionCursorQualified']);self.assertEqual(result['cursorProofPassed'],not fail)
                self.assertNotIn('core-publication',calls);self.assertNotIn('native-platform',calls)

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
                save(d/'challenges.json',dict(observations=rows,telemetry=[],prefix='rejectedAll',noResumeAfterEqualPolls=True));save(d/'trace.json',dict(events=[dict(kind='geometry')]))
        actions=['alpha','restore-cursor','alpha','hidden','restore-cursor','occlude','unocclude','move-away-back','move','move-back','minimize-restore','resize','close']
        save(root/'source-actions.json',dict(overflow=False,records=[dict(action=a,win32Succeeded=True) for a in actions]))
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
        for fault in [None,'missing-pair','missing-diagnostic','missing-exemplar','wrong-alpha','doubled-candidate','wrong-placement','second-exemplar','changed-observation','opaque-alpha','unchanged-wgc']:
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
                if fault=='changed-observation':
                    p=root/'alpha-with/geometry.json';info=json.loads(p.read_text());info['observations'][1]['position']=[49,48];info['observations'][1]['cursorInfoPosition']=[49,48];p.write_text(json.dumps(info))
                # Only the external FFmpeg decode is substituted; native raster, geometry, alpha and raw overlay checks run.
                with patch('cursor_check.movie',return_value=(clock,30)):
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
for i,name in enumerate(['without','with','alpha-without','alpha-with','challenge','closed','alpha-probe','unexpected','nested/alpha-with']):
    d=root/'session-0'/name;d.mkdir(parents=True,exist_ok=True)
    (d/'owned.json').write_text(json.dumps(dict(parent=os.getpid(),worker=100+i*2,encoder=101+i*2)))
    if i<6:gates.append(d/'handles-open')
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
            self.assertEqual([h.pid for h in handles],list(range(100,112)));self.assertTrue(all(h.closed for h in handles))
            receipt=json.loads((root/'cursor-borderless-even-command.json').read_text())
            self.assertEqual(len(receipt['ownership']),6);self.assertIsNone(receipt['error'])
            self.assertTrue(all(r['signaled']==[True,True] and r['fallback'] is False for r in receipt['ownership']))


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
