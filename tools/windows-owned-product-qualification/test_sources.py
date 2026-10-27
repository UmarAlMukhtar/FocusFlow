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
