import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import time

spec = importlib.util.spec_from_file_location('audio_stage', Path(__file__).with_name('stage.py'))
stage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stage)

class StageTests(unittest.TestCase):
    def test_host_observations_never_grant_permission(self):
        env = {'GITHUB_REPOSITORY': stage.REPOSITORY, 'GITHUB_REF': stage.SOURCE_REF,
               'GITHUB_WORKFLOW_REF': stage.WORKFLOW_REF, 'GITHUB_WORKFLOW_SHA': 'a'*40,
               'GITHUB_SHA': 'b'*40, 'GITHUB_RUN_ID': '10', 'GITHUB_RUN_ATTEMPT': '1',
               'GITHUB_JOB': 'native-discovery', 'RUNNER_NAME': 'Hosted Agent', 'ImageOS': 'win22',
               'RUNNER_ARCH': 'X64', 'RUNNER_OS': 'Windows', 'RUNNER_ENVIRONMENT': 'github-hosted',
               'GITHUB_ACTIONS': 'true'}
        observed = stage.host_observations(env)
        self.assertNotIn('approved', observed)
        self.assertEqual(observed['source_sha'], 'b'*40)
        for key in env:
            wrong = dict(env); wrong[key] = ''
            with self.subTest(key=key), self.assertRaises(ValueError): stage.host_observations(wrong)
        for key in ['GITHUB_REPOSITORY', 'GITHUB_REF', 'GITHUB_WORKFLOW_REF', 'RUNNER_OS', 'RUNNER_ENVIRONMENT']:
            wrong = dict(env); wrong[key] = 'attacker'
            with self.assertRaises(ValueError): stage.host_observations(wrong)

    def test_active_job_exact_run_observation(self):
        jobs = {'total_count': 1, 'jobs': [{'id': 45, 'name': 'native-discovery', 'status': 'in_progress', 'run_id': 10, 'run_attempt': 1, 'runner_name': 'Hosted Agent'}]}
        self.assertEqual(stage.select_active_job(jobs, 10, 1, 'Hosted Agent'), 45)
        for field, value in [('status', 'completed'), ('run_id', 11), ('run_attempt', 2), ('runner_name', 'other')]:
            changed=json.loads(json.dumps(jobs));changed['jobs'][0][field]=value
            with self.assertRaises(ValueError): stage.select_active_job(changed,10,1,'Hosted Agent')
        jobs['jobs']*=2
        with self.assertRaises(ValueError): stage.select_active_job(jobs,10,1,'Hosted Agent')

    def test_create_once_and_package_allowlist(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp);allowed=root/'allowed.json';stage.write_new(allowed,b'{}')
            with self.assertRaises(FileExistsError): stage.write_new(allowed,b'changed')
            (root/'secret.txt').write_text('credential')
            upload=root/'upload';stage.copy_allowlist(root,upload,['allowed.json'])
            self.assertEqual([p.name for p in upload.iterdir()],['allowed.json'])
            self.assertEqual((upload/'allowed.json').read_bytes(),b'{}')
            with self.assertRaises(ValueError):stage.copy_allowlist(root,root/'bad',['../secret.txt'])
            (root/'link').symlink_to(allowed)
            with self.assertRaises(ValueError):stage.copy_allowlist(root,root/'badlink',['link'])

    def test_pin_change_and_closure_missing_fail_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp);(root/'input.rs').write_text('a');pin=stage.identity(root/'input.rs',root)
            stage.verify_identity(root,pin);(root/'input.rs').write_text('b')
            with self.assertRaises(ValueError):stage.verify_identity(root,pin)
            dep=root/'output.d';dep.write_text('output: input.rs missing.rs\n')
            with self.assertRaises(ValueError):stage.compiler_inputs(root,[dep])
            dep.write_text('output: input.rs\n')
            self.assertEqual(stage.compiler_inputs(root,[dep])[0]['path'],'input.rs')
            dep.write_text('output: ../outside.rs\n')
            with self.assertRaises(ValueError):stage.compiler_inputs(root,[dep])

    def test_individual_command_exit_and_caps(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            good=stage.run_logged(root,'good',[sys.executable,'-c','print("ok")'],{},2,1024)
            self.assertEqual(good['exit_code'],0);self.assertTrue(good['exact_exit'])
            bad=stage.run_logged(root,'bad',[sys.executable,'-c','raise SystemExit(7)'],{},2,1024)
            self.assertEqual(bad['exit_code'],7)
            flood=stage.run_logged(root,'flood',[sys.executable,'-c','print("x"*100000)'],{},2,1024)
            self.assertEqual(flood['failure'],'output-cap');self.assertLessEqual((root/'flood.log').stat().st_size,1024)
            hang=stage.run_logged(root,'hang',[sys.executable,'-c','import time;time.sleep(10)'],{},0.05,1024)
            self.assertEqual(hang['failure'],'deadline');self.assertTrue(hang['exact_exit'])

    def test_controller_deadline_retains_child_and_records_failure_before_delayed_exit(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            # This child refuses to exit until the watchdog has durably failed the active work.
            code = 'from pathlib import Path; import time; p=Path(' + repr(str(root/'discovery-controller-failure.json')) + '); deadline=time.monotonic()+2\nwhile not p.exists() and time.monotonic()<deadline: time.sleep(.01)\nassert p.exists(); time.sleep(.05); raise SystemExit(71)'
            receipt=stage.run_controller(root,[sys.executable,'-c',code],{},b'{}\n',seconds=.05)
            self.assertEqual(receipt['failure'],'deadline')
            self.assertEqual(receipt['exit_code'],71)
            self.assertTrue(receipt['exact_exit'])
            failed=json.loads((root/'discovery-controller-failure.json').read_text())
            self.assertEqual(failed['status'],'Failed')
            self.assertTrue(failed['cleanup_pending'])
            self.assertFalse(failed['exact_exit'])

    def test_controller_watchdog_retains_uncertain_exact_child_until_job_teardown(self):
        class Teardown(Exception): pass
        class Pipe:
            def write(self, raw): pass
            def close(self): pass
            def flush(self): pass
            def read1(self, count): return b''
        class Child:
            stdin=Pipe(); stdout=Pipe()
            def poll(self): return None
            def kill(self): raise AssertionError('must retain exact controller')
            def wait(self, timeout=None): raise AssertionError('must keep polling without detaching')
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp); polls=[]
            def pause(_):
                polls.append(1)
                if len(polls)==5: raise Teardown()
            with patch.object(stage.subprocess,'Popen',return_value=Child()), patch.object(stage.time,'sleep',side_effect=pause):
                with self.assertRaises(Teardown):
                    stage.run_controller(root,['controller'],{},b'{}\n',seconds=0)
            self.assertEqual(len(polls),5)
            self.assertFalse((root/'discovery-controller.json').exists())
            failed=json.loads((root/'discovery-controller-failure.json').read_text())
            self.assertEqual(failed['status'],'Failed');self.assertTrue(failed['cleanup_pending'])

    def test_controller_overflow_retains_until_exact_exit_and_stays_failed(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            code='import sys,time; sys.stdout.write("x"*100000);sys.stdout.flush();time.sleep(.1);raise SystemExit(71)'
            receipt=stage.run_controller(root,[sys.executable,'-c',code],{},b'{}\n',seconds=2,cap=1024)
            self.assertEqual(receipt['failure'],'output-cap')
            self.assertEqual(receipt['exit_code'],71)
            self.assertLessEqual((root/'discovery-controller.log').stat().st_size,1024)

    def test_controller_request_pipe_stays_live_until_watchdog_closes_it(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp)
            code='import sys; assert sys.stdin.buffer.readline()==b"{}\\n"; assert sys.stdin.buffer.read()==b""; raise SystemExit(71)'
            receipt=stage.run_controller(root,[sys.executable,'-c',code],{},b'{}\n',seconds=.08)
            self.assertEqual(receipt['failure'],'deadline');self.assertEqual(receipt['exit_code'],71)
            self.assertGreaterEqual(receipt['elapsed_seconds'],.08)

    def test_controller_bad_frame_never_starts_process(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(stage.subprocess,'Popen',side_effect=AssertionError('must not launch')):
            for raw in [b'{}',b'{}\n{}\n',b'x'*4096+b'\n']:
                with self.assertRaises(ValueError): stage.run_controller(Path(temp),['unused'],{},raw)

    def test_controller_pipe_and_observation_errors_retain_until_exact_exit(self):
        for source in ['read','write','observe']:
            with self.subTest(source=source), tempfile.TemporaryDirectory() as temp:
                root=Path(temp); count=[0]; closed=[False]
                class Pipe:
                    def write(self, raw):
                        if source=='write': raise OSError('write failed')
                    def flush(self): pass
                    def close(self): closed[0]=True
                    def read1(self, size):
                        if source=='read': raise OSError('read failed')
                        return b''
                class Child:
                    stdin=Pipe();stdout=Pipe()
                    def poll(self):
                        count[0]+=1
                        if count[0]<5:
                            if source=='observe': raise OSError('observation failed')
                            return None
                        self.exited=True
                        return 71
                    def kill(self): raise AssertionError('must retain')
                with patch.object(stage.subprocess,'Popen',return_value=Child()):
                    receipt=stage.run_controller(root,['controller'],{},b'{}\n',seconds=.01)
                self.assertTrue(closed[0]);self.assertEqual(receipt['exit_code'],71)
                self.assertTrue(receipt['failure']);self.assertTrue(receipt['exact_exit'])

    def test_controller_log_sink_failure_keeps_draining_to_exact_exit(self):
        class TestTeardown(Exception): pass
        real_open=Path.open; real_popen=stage.subprocess.Popen; real_sleep=time.sleep
        for sink_error in ['open','write']:
            with self.subTest(sink_error=sink_error), tempfile.TemporaryDirectory() as temp:
                root=Path(temp); attempts=[]; children=[]; cleanup=[]
                # Exceeds normal anonymous-pipe capacity; exit also requires watchdog EOF.
                code='import sys; sys.stdout.buffer.write(b"x"*(2*1024*1024)); sys.stdout.buffer.flush(); assert sys.stdin.buffer.readline()==b"{}\\n"; assert sys.stdin.buffer.read()==b""; raise SystemExit(71)'
                class FailedSink:
                    def __enter__(self): return self
                    def __exit__(self, *args): pass
                    def write(self, raw): raise OSError('injected log write failure')
                def open_file(path, mode='r', *args, **kwargs):
                    if path==root/'discovery-controller.log' and mode=='ab':
                        attempts.append(1)
                        if sink_error=='open': raise OSError('injected log open failure')
                        return FailedSink()
                    return real_open(path,mode,*args,**kwargs)
                def spawn(*args, **kwargs):
                    child=real_popen(*args,**kwargs); children.append(child); cleanup.append(child.kill)
                    def forbidden_kill(): raise AssertionError('watchdog must retain the exact controller')
                    child.kill=forbidden_kill
                    return child
                until=time.monotonic()+3
                def pause(_):
                    if time.monotonic()>=until: raise TestTeardown('drain stopped before exact exit')
                    real_sleep(.005)
                try:
                    with patch.object(Path,'open',open_file), patch.object(stage.subprocess,'Popen',spawn), patch.object(stage.time,'sleep',pause):
                        try:
                            receipt=stage.run_controller(root,[sys.executable,'-c',code],{},b'{}\n',seconds=2,cap=64*1024)
                        except TestTeardown:
                            self.fail('Log sink failure stranded the controller before its exact exit')
                    self.assertEqual(len(attempts),1,'failed sink must never be retried')
                    self.assertEqual(receipt['failure'],'log-write')
                    self.assertEqual(receipt['exit_code'],71);self.assertTrue(receipt['exact_exit'])
                    failed=json.loads((root/'discovery-controller-failure.json').read_text())
                    self.assertEqual(failed['status'],'Failed');self.assertTrue(failed['cleanup_pending'])
                    self.assertLessEqual((root/'discovery-controller.log').stat().st_size,64*1024)
                finally:
                    # Only the test harness tears down a stranded RED child after its bounded observation.
                    for child,kill in zip(children,cleanup):
                        if child.poll() is None: kill()
                        child.wait(timeout=2)
                        child.stdin.close();child.stdout.close()

    def test_compiler_relative_include_uses_manifest_directory(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp);manifest=root/'worker';(manifest/'src').mkdir(parents=True)
            (manifest/'src/lib.rs').write_text('lib');(root/'fetch.ps1').write_text('fetch')
            dep=root/'output.d';dep.write_text('output: src/lib.rs src/../../fetch.ps1\n')
            paths=stage.compiler_inputs(root,[dep],manifest)
            self.assertEqual([p['path'] for p in paths],['fetch.ps1','worker/src/lib.rs'])

    def test_controller_workspace_matches_rust_canonical_prefix(self):
        self.assertEqual(stage.controller_workspace(r'D:\a\FocusFlow\FocusFlow'),r'\\?\D:\a\FocusFlow\FocusFlow')
        with self.assertRaises(ValueError):stage.controller_workspace('relative')

    def test_manifest_not_a_staging_input(self):
        with self.assertRaises(SystemExit):stage.arguments(['discover','--manifest','local.json'])
        with self.assertRaises(SystemExit):stage.arguments(['build','--repository','other'])

if __name__=='__main__':unittest.main()
