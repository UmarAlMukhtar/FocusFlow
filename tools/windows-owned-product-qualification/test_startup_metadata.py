"""Private scalar-only startup diagnostic controls; no native capture or input."""
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import run

BRANCH='codex-windows-startup-metadata'
def context():
    return dict(GITHUB_ACTIONS='true',RUNNER_ENVIRONMENT='github-hosted',RUNNER_OS='Windows',GITHUB_REPOSITORY='UmarAlMukhtar/FocusFlow',GITHUB_REF='refs/heads/'+BRANCH,GITHUB_EVENT_NAME='push',GITHUB_WORKFLOW_REF='UmarAlMukhtar/FocusFlow/.github/workflows/windows-owned-product-qualification.yml@refs/heads/'+BRANCH,GITHUB_SHA='a'*40,GITHUB_RUN_ID='123',GITHUB_RUN_ATTEMPT='1',FOCUSFLOW_METADATA_REVIEWED_SHA='a'*40,FOCUSFLOW_METADATA_REVIEWED_REPOSITORY='UmarAlMukhtar/FocusFlow',FOCUSFLOW_METADATA_REVIEWED_REF='refs/heads/'+BRANCH,FOCUSFLOW_METADATA_REVIEWED_RUN='123:1')

class MetadataContext(unittest.TestCase):
    def test_exact_positive_binding_and_every_missing_value_fail_closed(self):
        guard=getattr(run,'require_metadata_disposable_context',None);self.assertIsNotNone(guard,'metadata-only positive context guard missing')
        good=context()
        with patch.dict(os.environ,good,clear=True):self.assertEqual(guard()['ref'],'refs/heads/'+BRANCH)
        for key in good:
            for value in [None,'wrong']:
                bad=good.copy()
                if value is None:bad.pop(key)
                else:bad[key]=value
                with self.subTest(key=key,value=value),patch.dict(os.environ,bad,clear=True),self.assertRaises(AssertionError):guard()
        with patch.dict(os.environ,context(),clear=True),self.assertRaises(AssertionError):run.require_cursor_disposable_context()

    def test_metadata_source_requires_metadata_guard_before_spawn(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);case=root/'session-0';case.mkdir();run.write(case/'source-request.json',dict(width=320,height=240,nonce=601,animated=False,privateProfile='startup-metadata'))
            owner=run.GeneratedSources('harness',root,None)
            with patch.dict(os.environ,{},clear=True),patch('run.subprocess.Popen') as spawn,self.assertRaises(AssertionError):owner.poll(999)
            spawn.assert_not_called();self.assertFalse((case/'source.log').exists())

    def test_package_names_metadata_only_and_rejects_media_or_authority(self):
        from package_evidence import package
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);ev=root/'evidence';case=ev/'startup-metadata/session-0';probe=case/'startup-probe';probe.mkdir(parents=True)
            for n in ['startup-metadata.json','startup-phase.json','metadata-owned.json','metadata-cleanup.json']:(probe/n).write_text('{}')
            (probe/'unknown.json').write_text('{}');(case/'source-request.json').write_text('{}');(ev/'startup-metadata-command.json').write_text('{}')
            hashes=package(ev,root/'none',root/'qualification-upload')
            expected={f'cases/startup-metadata/session-0/startup-probe/{n}' for n in ['startup-metadata.json','startup-phase.json','metadata-owned.json','metadata-cleanup.json']}|{'cases/startup-metadata/session-0/source-request.json','startup-metadata-command.json'}
            self.assertEqual(set(hashes),expected)
            workflow=(Path(__file__).resolve().parents[2]/'.github/workflows/windows-owned-product-qualification.yml').read_text().split('name: windows-startup-metadata-',1)[1]
            globs=[line.strip() for line in workflow.splitlines() if line.strip().startswith('qualification-upload/')]
            uploaded={p.relative_to(root/'qualification-upload').as_posix() for glob in globs for p in root.glob(glob) if p.is_file()}
            self.assertTrue(expected<=uploaded)
            for n in ['screen.mp4','raw-0.pam','capture.json','capture.windows-v2','project.json']:
                p=probe/n;p.write_bytes(b'forbidden')
                with self.subTest(name=n),self.assertRaises(RuntimeError):package(ev,root/'none',root/('bad-'+n))
                p.unlink()

def scalar_fixture():
    def row(phase,q0,size,content=None,texture=None):return dict(phase=phase,q0=q0,q1=q0+10,itemIdentity='GraphicsCaptureItem(0x123)',itemSize=size,contentSize=content,textureSize=texture,native100ns=q0 if phase=='callback' else None,readErrors=[])
    return dict(metadataOnly=True,selectedHwnd=12345,sourcePid=42,sourceThread=84,itemIdentity='GraphicsCaptureItem(0x123)',sameItemClone=True,frequency=10000000,rowLimit=32,collectionLimitMs=2000,rows=[row('beforeStart',100,[336,279]),row('afterStart',200,[336,279]),row('callback',300,[336,279],[322,272],[336,279]),row('callback',400,[322,272],[322,272],[322,272])],callbacks=2,dropped=0,stopReason='collectionDeadline',captureClosed=False,stopComplete=True,stopError=None,collectionError=None,startupError=None,readErrorCount=0)

class ScalarEvidence(unittest.TestCase):
    def test_mismatch_is_observed_metadata_and_never_recording_admission(self):
        import startup_check as s
        result=s.scalar_rows(scalar_fixture());self.assertTrue(result['itemSizeChanged']);self.assertEqual([r['contentEqualsTexture'] for r in result['callbackSizes']],[False,True]);self.assertFalse(result['recordingQualified'])
        for fault in ['epoch','pixels','identity','read-error','over2s','rows33','no-callback','missing-phase','wrong-frequency','incomplete-stop','forced-source-end']:
            value=scalar_fixture()
            if fault in ['epoch','pixels']:value[fault]=None
            elif fault=='identity':value['rows'][2]['itemIdentity']='other object'
            elif fault=='read-error':value['readErrorCount']=1;value['rows'][2]['readErrors']=['item.Size failed']
            elif fault=='over2s':value['rows'][3]['q1']=value['rows'][2]['q0']+20000001
            elif fault=='rows33':value['rows']+=value['rows'][2:3]*29;value['callbacks']=31
            elif fault=='no-callback':value['rows']=value['rows'][:2];value['callbacks']=0
            elif fault=='missing-phase':value['rows'][1]['phase']='unexpected'
            elif fault=='wrong-frequency':value['frequency']=0
            elif fault=='incomplete-stop':value['stopComplete']=False
            elif fault=='forced-source-end':value['stopReason']='sourceUnavailable'
            with self.subTest(fault=fault),self.assertRaises(AssertionError):s.scalar_rows(value)

    def test_connected_receipts_and_explicit_metadata_caps(self):
        import startup_check as s
        from package_evidence import package
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);case=root/'evidence/startup-metadata/session-0';probe=case/'startup-probe';probe.mkdir(parents=True)
            def save(path,value):path.write_text(json.dumps(value))
            save(probe/'startup-metadata.json',scalar_fixture());save(probe/'startup-phase.json',dict(phase='finished',qpc=1000));owned=dict(parent=999,worker=101,qpc=10);save(probe/'metadata-owned.json',owned)
            save(probe/'metadata-cleanup.json',dict(worker=101,workerExit=0,workerSignaled=True,fallback=False,error=None))
            save(case/'source-selection.json',dict(driverPid=999,sourcePid=42,hwnd=12345));save(case/'source-cleanup.json',dict(pid=42,observed=True,exitCode=0,signaled=True,fallback=False));save(case/'source-actions.json',dict(overflow=False,records=[dict(action='close',win32Succeeded=True)]))
            command=root/'command.json';receipt=dict(exitCode=0,error=None,ownership=[dict(**owned,signaled=[True],fallback=False)]);save(command,receipt)
            result=s.check(case,command);self.assertTrue(result['noEncoder']);self.assertFalse(result['productionCursorQualified']);self.assertTrue((probe/'startup-analysis.json').exists())
            receipt['ownership'][0]['encoder']=102;save(command,receipt)
            with self.assertRaises(AssertionError):s.check(case,command)
            (probe/'startup-metadata.json').write_bytes(b'x'*(65536+1))
            with self.assertRaises(AssertionError):run.bound(root/'evidence')
            with self.assertRaises(RuntimeError):package(root/'evidence',root/'none',root/'upload')

class MetadataRouting(unittest.TestCase):
    def test_scope_builds_only_harness_and_one_metadata_case(self):
        from types import SimpleNamespace
        for fail in [False,True]:
            with self.subTest(fail=fail),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);repo=Path(__file__).resolve().parents[2];args=SimpleNamespace(scope='startup-metadata',evidence=root/'evidence',ffmpeg=root/'ffmpeg',ffprobe=root/'ffprobe');builds=[];calls=[]
                def build(repo,evidence,manifest,name,test):builds.append((name,test));return root/'harness'
                def execute(argv,evidence,name,*unused,**kwargs):
                    calls.append((argv,name));self.assertEqual(name,'startup-metadata');self.assertEqual(argv[1],'--startup-metadata')
                    if fail:raise RuntimeError('metadata worker failed')
                metadata={'packages':[{'name':'windows-capture','source':None,'manifest_path':str(repo/'FocusFlow/vendor/windows-capture/Cargo.toml')}]}
                with patch('run.sys.platform','win32'),patch.dict(os.environ,context(),clear=True),patch('run.os.chdir'),patch('run.source_manifest',return_value={}),patch('run.sha',side_effect=lambda p:run.FFMPEG_SHA if p==args.ffmpeg else run.FFPROBE_SHA if p==args.ffprobe else 'harness'),patch('run.subprocess.check_output',side_effect=lambda argv,**kw:json.dumps(metadata) if argv[0]=='cargo' else 'a'*40),patch('run.build_exe',side_effect=build),patch('run.execute',side_effect=execute),patch('startup_check.check') as check:
                    if fail:
                        with self.assertRaises(RuntimeError):run.run(args)
                    else:run.run(args)
                self.assertEqual(builds,[('windows_owned_qualification',True)]);self.assertEqual(len(calls),1);self.assertEqual(check.call_count,0 if fail else 1)
                result=json.loads((args.evidence/'result.json').read_text());self.assertFalse(result['passed']);self.assertTrue(result['diagnosticOnly']);self.assertEqual(result['metadataCollected'],not fail);self.assertFalse(result['productionCursorQualified'])

    def test_missing_context_stops_before_tools_or_output(self):
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);args=SimpleNamespace(scope='startup-metadata',evidence=root/'evidence',ffmpeg=root/'f',ffprobe=root/'p')
            with patch('run.sys.platform','win32'),patch.dict(os.environ,{},clear=True),patch('run.subprocess.Popen') as popen,patch('run.subprocess.check_output') as output,self.assertRaises(AssertionError):run.run(args)
            popen.assert_not_called();output.assert_not_called();self.assertFalse(args.evidence.exists())
            with patch.dict(os.environ,{},clear=True),patch('run.subprocess.Popen') as direct,self.assertRaises(AssertionError):run.execute(['unused'],root,'startup-metadata')
            direct.assert_not_called();self.assertFalse((root/'startup-metadata.log').exists())
            with patch('run.sys.platform','win32'),patch.dict(os.environ,context(),clear=True),patch('run.os.chdir'),patch('run.subprocess.check_output',return_value='b'*40) as checkout,self.assertRaises(AssertionError):run.run(args)
            checkout.assert_called_once_with(['git','rev-parse','HEAD'],text=True,encoding='ascii');self.assertFalse(args.evidence.exists())

    def test_exact_observer_opens_worker_only_and_ignores_unowned_probe_paths(self):
        import sys
        driver='''import json,os,sys,time
from pathlib import Path
root=Path(sys.argv[1]);d=root/'session-0/startup-probe';d.mkdir(parents=True)
(d/'metadata-owned.json').write_text(json.dumps(dict(parent=os.getpid(),worker=101,qpc=10)))
u=root/'session-0/unexpected';u.mkdir();(u/'metadata-owned.json').write_text(json.dumps(dict(parent=os.getpid(),worker=999,qpc=10)))
while not (d/'metadata-handles-open').exists():time.sleep(.005)
'''
        opened=[]
        class Handle:
            def __init__(self,pid):self.pid=pid;self.closed=False;opened.append(self)
            def signaled(self):return True
            def close(self):self.closed=True
            def kill(self):raise AssertionError('successful observer must not kill')
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);script=root/'driver.py';script.write_text(driver)
            with patch.dict(os.environ,context(),clear=True),patch('run.ExactHandle',Handle),patch('run.native_qpc',return_value=100):run.execute([sys.executable,script,root/'case'],root,'startup-metadata',3,observe=root/'case')
            self.assertEqual([h.pid for h in opened],[101]);self.assertTrue(all(h.closed for h in opened));receipt=json.loads((root/'startup-metadata-command.json').read_text());self.assertIsNone(receipt['error']);self.assertEqual(receipt['ownership'][0]['signaled'],[True]);self.assertNotIn('encoder',receipt['ownership'][0])
