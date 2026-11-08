"""Real owned child processes exercise fixture control, never stand in for Windows WGC/Jobs."""
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
import run
from package_evidence import package

SOURCE = '''import json,os,sys,time
from pathlib import Path
p=Path(sys.argv[2])
if (p/'fail-start').exists():sys.exit(7)
pid=os.getpid()+int((p/'wrong-pid').exists())
(p/'source-ready.pending').write_text(json.dumps(dict(pid=pid,hwnd=123)),encoding='utf-8')
(p/'source-ready.pending').replace(p/'source-ready.json')
while not (p/'source-command-1.json').exists():time.sleep(.005)
(p/'paint-history.json').write_text('[[2,100]]',encoding='utf-8')
'''

class Sources(unittest.TestCase):
    def setup_owner(self, root):
        script=root/'source.py';script.write_text(SOURCE,encoding='utf-8')
        original=run.subprocess.Popen
        def spawn(argv,**kwargs):
            self.assertEqual(argv[1],'--source')
            return original([sys.executable,str(script),*argv[1:]],**kwargs)
        return patch('run.subprocess.Popen',side_effect=spawn)

    def request(self,root,i=0):
        case=root/f'session-{i}';case.mkdir()
        run.write(case/'source-request.json',dict(width=641,height=359,nonce=100+i,animated=True))
        return case

    def wait(self,owner,condition):
        end=time.monotonic()+3
        while not condition():
            self.assertLess(time.monotonic(),end)
            owner.poll(os.getpid());time.sleep(.005)

    def test_repeated_owned_source_handshake_and_close(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);owner=run.GeneratedSources('harness',root,None)
            with self.setup_owner(root),patch('run.window_pid',side_effect=lambda hwnd:next(v['proc'].pid for v in owner.children.values() if v['proc'].poll() is None)):
                try:
                    for i in range(3):
                        case=self.request(root,i);self.wait(owner,lambda:(case/'source-observed.json').exists())
                        receipt=json.loads((case/'source-observed.json').read_text())
                        self.assertNotEqual(receipt['pid'],os.getpid());self.assertEqual(receipt['hwnd'],123)
                        run.fixture_write(case/'source-command-1.json',dict(sequence=1,action='close'));self.wait(owner,lambda:(case/'source-exit.json').exists())
                        self.assertEqual(json.loads((case/'source-exit.json').read_text())['exitCode'],0)
                finally:owner.cleanup(False)
            self.assertEqual(len(owner.children),3)
            self.assertTrue(all(x['proc'].poll()==0 for x in owner.children.values()))

    def test_bad_start_or_wrong_identity_reaps_owned_child(self):
        for flag in ['fail-start','wrong-pid']:
            with self.subTest(flag=flag),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);case=self.request(root);(case/flag).touch();owner=run.GeneratedSources('harness',root,None)
                with self.setup_owner(root),patch('run.window_pid',return_value=123):
                    try:
                        with self.assertRaises(AssertionError):self.wait(owner,lambda:False)
                    finally:owner.cleanup(True)
                self.assertFalse((case/'source-observed.json').exists())
                self.assertTrue(all(x['proc'].poll() is not None for x in owner.children.values()))

    def test_source_must_survive_causal_gate_and_error_cleanup(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);case=self.request(root);owner=run.GeneratedSources('harness',root,None)
            with self.setup_owner(root),patch('run.window_pid',side_effect=lambda hwnd:owner.children[case]['proc'].pid):
                try:
                    self.wait(owner,lambda:(case/'source-observed.json').exists())
                    with patch('run.native_qpc',side_effect=[11,22]):
                        owner.require_alive('aliveBeforeParentKillQpc');owner.require_alive('aliveAfterProductExitQpc')
                    self.assertEqual(owner.children[case]['aliveBeforeParentKillQpc'],11)
                    self.assertEqual(owner.children[case]['aliveAfterProductExitQpc'],22)
                    self.assertFalse((case/'source-close').exists())
                    owner.children[case]['proc'].kill();owner.children[case]['proc'].wait(timeout=3)
                    with self.assertRaises(AssertionError):owner.require_alive()
                finally:owner.cleanup(True)
            self.assertTrue(json.loads((case/'source-cleanup.json').read_text())['signaled'])

    def test_controls_are_bounded_and_only_named_source_receipts_upload(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);ev=root/'evidence';case=ev/'geometry-320x240/session-0';case.mkdir(parents=True)
            names=['source-request.json','source-ready.json','source-observed.json','source-selection.json','source-exit.json','source-cleanup.json','source-command-1.json','source-command-2.json','source-ack-1.json','source-ack-2.json']
            for name in names:run.fixture_write(case/name,dict(sequence=1,action='close'))
            (case/'source.log').write_text('owned source only');(case/'source-private.json').write_text('not allowed');(case/'source-command-3.json').write_text('{}')
            hashes=package(ev,root/'absent',root/'upload')
            self.assertEqual(set(hashes),{f'cases/geometry-320x240/session-0/{n}' for n in names+['source.log']})
            (case/'oversized.json').write_bytes(b' '*1025)
            with self.assertRaises(AssertionError):run.fixture_read(case/'oversized.json')

    def test_outer_error_and_unclosed_source_are_reaped(self):
        for error in [False,True]:
            with self.subTest(error=error),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);case=self.request(root);owner=run.GeneratedSources('harness',root,None)
                with self.setup_owner(root),patch('run.window_pid',side_effect=lambda hwnd:owner.children[case]['proc'].pid):
                    self.wait(owner,lambda:(case/'source-observed.json').exists())
                    if error:owner.cleanup(True)
                    else:
                        with self.assertRaises(AssertionError):owner.cleanup(False)
                receipt=json.loads((case/'source-cleanup.json').read_text())
                self.assertTrue(receipt['signaled']);self.assertEqual(receipt['fallback'],not error)

    def test_execute_watchdog_and_driver_error_cleanup_sources(self):
        driver_code="""import json,sys,time
from pathlib import Path
root=Path(sys.argv[1]);case=root/'session-0';case.mkdir(parents=True)
(case/'source-request.pending').write_text(json.dumps(dict(width=641,height=359,nonce=100,animated=True)),encoding='utf-8')
(case/'source-request.pending').replace(case/'source-request.json')
while not (case/'source-observed.json').exists():time.sleep(.005)
if sys.argv[2]=='error':sys.exit(7)
time.sleep(10)
"""
        for mode in ['error','watchdog']:
            with self.subTest(mode=mode),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);observe=root/'case';driver=root/'driver.py';driver.write_text(driver_code)
                source=root/'source.py';source.write_text(SOURCE);original=run.subprocess.Popen
                def spawn(argv,**kw):
                    if argv[1]=='--source':argv=[sys.executable,str(source),*argv[1:]]
                    return original(argv,**kw)
                def owner(hwnd):return json.loads((observe/'session-0/source-ready.json').read_text())['pid']
                with patch('run.subprocess.Popen',side_effect=spawn),patch('run.window_pid',side_effect=owner):
                    with self.assertRaises(RuntimeError):run.execute([sys.executable,driver,observe,mode],root,'admission-fixture',3,observe=observe)
                cleanup=json.loads((observe/'session-0/source-cleanup.json').read_text())
                self.assertTrue(cleanup['signaled']);self.assertEqual(cleanup['reason'],'error')
                receipt=json.loads((root/'admission-fixture-command.json').read_text())
                self.assertIn('outer watchdog' if mode=='watchdog' else 'command exit 7',receipt['error'])

if __name__=='__main__':unittest.main()


class ProductionSources(unittest.TestCase):
    wait=Sources.wait
    request=Sources.request
    def test_production_discovers_only_fixed_source_layout(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)/'production-popup-even-r0';root.mkdir();case=root/'source';case.mkdir()
            run.write(case/'source-request.json',dict(width=320,height=240,nonce=700,animated=False,privateProfile='borderless'))
            self.request(root)
            owner=run.GeneratedSources('harness',root,None,production_static=True)
            original=run.subprocess.Popen;script=root/'source.py';script.write_text(SOURCE)
            def spawn(argv,**kwargs):
                self.assertEqual(argv[1],'--source-private');self.assertEqual(argv[-1],'borderless')
                return original([sys.executable,str(script),*argv[1:]],**kwargs)
            with patch('run.subprocess.Popen',side_effect=spawn),patch('run.require_cursor_disposable_context'),patch('run.window_pid',side_effect=lambda hwnd:owner.children[case]['proc'].pid):
                try:
                    self.wait(owner,lambda:(case/'source-observed.json').exists())
                    self.assertEqual(set(owner.children),{case})
                    run.fixture_write(case/'source-command-1.json',dict(sequence=1,action='close'));self.wait(owner,lambda:(case/'source-exit.json').exists())
                finally:owner.cleanup(False)

    def test_production_bad_tuple_or_link_never_launches(self):
        for failure in ['nonce','profile','layout','link']:
            with self.subTest(failure=failure),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp)/'production-popup-even-r0';root.mkdir();case=root/'source';case.mkdir()
                data=dict(width=320,height=240,nonce=700,animated=False,privateProfile='borderless')
                if failure=='nonce':data['nonce']=704
                if failure=='profile':data['privateProfile']='decorated'
                run.write(case/'source-request.json',data)
                if failure=='layout':root=root.rename(root.with_name('foreign-case'));case=root/'source'
                if failure=='link':
                    (case/'source-request.json').rename(case/'foreign.json');(case/'source-request.json').symlink_to(case/'foreign.json')
                owner=run.GeneratedSources('harness',root,None,production_static=True)
                with patch('run.require_cursor_disposable_context'),patch('run.subprocess.Popen',side_effect=AssertionError('must not launch')) as spawn:
                    with self.assertRaises(AssertionError):owner.poll(os.getpid())
                    spawn.assert_not_called()


class ProductionExecute(unittest.TestCase):
    def exercise(self, non_atomic=False):
        # Real portable children exercise owner orchestration; virtual handles are not Win32 proof.
        code=r'''import json,os,sys,time
from pathlib import Path
root=Path(sys.argv[1]);source=root/'source';source.mkdir(parents=True);session=root/'session-0';session.mkdir()
def put(path,value):
 p=root.parent/'owned-fixture.pending' if path.name=='owned.json' else path.with_suffix('.pending');p.write_text(json.dumps(value));p.replace(path)
def wait(path):
 end=time.monotonic()+5
 while not path.exists():
  if time.monotonic()>end:raise RuntimeError(str(path))
  time.sleep(.005)
put(source/'source-request.json',dict(width=320,height=240,nonce=700,animated=False,privateProfile='borderless'))
wait(source/'source-observed.json');ready=json.loads((source/'source-observed.json').read_text());identity=dict(hwnd=ready['hwnd'],sourcePid=ready['pid'])
put(source/'source-selection.json',dict(driverPid=os.getpid(),**identity));put(root/'production-source-ready.json',identity)
wait(root/'production-start-authorized.json');assert json.loads((root/'production-start-authorized.json').read_text())==dict(action='static-owned-canvas-v1',**identity)
put(session/'owned.json',dict(parent=os.getpid(),worker=33,encoder=44,qpc=3));wait(session/'handles-open')
put(root/'production-armed.json',dict(**identity,profile='pmv2_96_single_monitor_popup',candidateOnly=True))
for i in [0,1]:(session/f'raw-{i}.pam').write_bytes(b'P7\nWIDTH 320\nHEIGHT 240\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n'+b'\x60'*(320*240*4))
wait(root/'production-actions-complete.json');assert json.loads((root/'production-actions-complete.json').read_text())==dict(action='static-hold-complete-v1',**identity)
(root.parent/'worker-ended').touch();put(source/'source-command-1.json',dict(sequence=1,action='close'));wait(source/'source-exit.json')
put(root/'production-phase.json',dict(phase='postcapture',**identity,sourceClosed=True,workerEncoderJoined=True,qpc=123))
(session/'screen.mp4').write_bytes(b'portable orchestration fixture, no media claim');(session/'before-publish').touch();wait(session/'media-hashed')
'''
        if non_atomic:
            code=code.replace("put(session/'owned.json',dict(parent=os.getpid(),worker=33,encoder=44,qpc=3));wait(session/'handles-open')", "raw=json.dumps(dict(parent=os.getpid(),worker=33,encoder=44,qpc=3)).encode()\nwith (session/'owned.json').open('wb') as owned:\n time.sleep(.08);owned.write(raw[:12]);owned.flush();time.sleep(.08);owned.write(raw[12:])\nwait(session/'handles-open')")
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);observe=root/'production-popup-even-r0';driver=root/'driver.py';driver.write_text(code)
            source=root/'source.py';source.write_text(SOURCE);original=run.subprocess.Popen;read=run.production_read;incomplete=[]
            def read_control(path,*args,**kwargs):
                try:return read(path,*args,**kwargs)
                except json.JSONDecodeError as error:
                    if path.name=='owned.json':incomplete.append(error.doc)
                    raise
            def spawn(argv,**kw):
                if argv[1]=='--source-private':argv=[sys.executable,str(source),*argv[1:]]
                return original(argv,**kw)
            class Handle:
                def __init__(self,pid):
                    self.pid=pid
                    data=json.loads((observe/'session-0/owned.json').read_text());assert pid in [data['worker'],data['encoder']]
                def signaled(self):return (observe.parent/'worker-ended').exists()
                def kill(self):raise AssertionError('normal orchestration must not kill')
                def close(self):pass
            def owner(hwnd):return json.loads((observe/'source/source-ready.json').read_text())['pid']
            with patch('run.require_cursor_disposable_context'),patch('run.subprocess.Popen',side_effect=spawn),patch('run.window_pid',side_effect=owner),patch('run.ExactHandle',Handle),patch('run.native_qpc',return_value=123),patch('run.production_read',side_effect=read_control):
                run.execute([sys.executable,driver,observe],root,observe.name,10,observe=observe,production_static=True)
            receipt=root/(observe.name+'-command.json');run.require_production_cleanup(observe,receipt,complete=True)
            saved=json.loads(receipt.read_text());self.assertEqual(saved['productionStaticPhase']['phase'],'postcapture');self.assertFalse(saved['cleanupUnknown'])
            self.assertTrue(json.loads((observe/'session-0/media-preservation.json').read_text())['unchanged'])
            if non_atomic:
                self.assertIn('',incomplete);self.assertTrue(any(text.strip() for text in incomplete))

    def test_real_fake_children_complete_source_close_and_media_hash_handshakes(self):
        self.exercise()

    def test_delayed_nonatomic_initial_owned_write_settles_before_handles_open(self):
        self.exercise(non_atomic=True)


class ProductionOwnershipBoundary(unittest.TestCase):
    def test_foreign_ownership_control_never_opens_process_handles(self):
        from types import SimpleNamespace
        code="""import json,os,sys,time
from pathlib import Path
root=Path(sys.argv[1]);root.mkdir();session=root/'session-0'
foreign=root.parent/'foreign-session';foreign.mkdir();(foreign/'owned.json').write_text(json.dumps(dict(parent=os.getpid(),worker=33,encoder=44,qpc=3)))
session.symlink_to(foreign,target_is_directory=True)
time.sleep(5)
"""
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);observe=root/'production-popup-even-r0';driver=root/'driver.py';driver.write_text(code)
            owner=SimpleNamespace(children={},controls={},poll=lambda pid:None,cleanup=lambda *a,**kw:None,require_alive=lambda:None)
            with patch('run.require_cursor_disposable_context'),patch('run.GeneratedSources',return_value=owner),patch('run.ExactHandle',side_effect=AssertionError('must not open foreign handles')) as handles:
                with self.assertRaises(RuntimeError):run.execute([sys.executable,driver,observe],root,observe.name,2,observe=observe,production_static=True)
                handles.assert_not_called()
            self.assertTrue(json.loads((root/(observe.name+'-command.json')).read_text())['cleanupUnknown'])


class InitialOwnershipStrictness(unittest.TestCase):
    def test_complete_invalid_controls_fail_without_handles_and_incomplete_wait_is_bounded(self):
        from types import SimpleNamespace
        code="""import json,os,sys,time
from pathlib import Path
root=Path(sys.argv[1]);session=root/'session-0';session.mkdir(parents=True);kind=sys.argv[2]
data=dict(parent=os.getpid(),worker=33,encoder=44,qpc=3)
if kind=='schema':data['extra']=True
if kind=='parent':data['parent']=999
raw=json.dumps(data).encode()
if kind=='malformed':raw=b'{"parent":1,}'
if kind=='duplicate':raw=b'{"parent":1,"parent":1}'
if kind=='cap':raw=b' '*4097
if kind=='incomplete':raw=b'{"encoder":'
if kind=='link':
 target=root.parent/'foreign.json';target.write_bytes(raw);(session/'owned.json').symlink_to(target)
else:(session/'owned.json').write_bytes(raw)
time.sleep(10)
"""
        for kind in ['malformed','duplicate','schema','parent','cap','link','incomplete']:
            with self.subTest(kind=kind),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);observe=root/'production-popup-even-r0';driver=root/'driver.py';driver.write_text(code);reads=[];read=run.production_read
                source=SimpleNamespace(pid=22,poll=lambda:0)
                owner=SimpleNamespace(children={observe/'source':dict(proc=source,observed=True)},controls={},poll=lambda pid:None,cleanup=lambda *a,**kw:None,require_alive=lambda:None)
                def reading(path,*args,**kwargs):
                    if path.name=='owned.json':reads.append(True)
                    return read(path,*args,**kwargs)
                with patch('run.require_cursor_disposable_context'),patch('run.GeneratedSources',return_value=owner),patch('run.poll_production_static'),patch('run.production_read',side_effect=reading),patch('run.ExactHandle',side_effect=AssertionError('no handle for invalid/incomplete owner')) as handles:
                    with self.assertRaises(RuntimeError):run.execute([sys.executable,driver,observe,kind],root,observe.name,8,observe=observe,production_static=True)
                    handles.assert_not_called()
                receipt=json.loads((root/(observe.name+'-command.json')).read_text());self.assertTrue(receipt['cleanupUnknown']);self.assertTrue(receipt['cleanup']['parent']['signaled'])
                if kind=='incomplete':self.assertIn('initial ownership publication deadline',receipt['error']);self.assertGreater(len(reads),1)
                else:self.assertLessEqual(len(reads),1)
