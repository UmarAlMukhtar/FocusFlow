import copy
import json
from pathlib import Path
import re
import tempfile
import unittest
import run
from types import SimpleNamespace
from unittest.mock import patch


STARTUP_UNIT_LOGS={
    'cursor-startup-lifecycle': '\n'.join('test recorder::windows_capture::lifecycle::tests::'+name+' ... ok' for name in [
        'metadata_only_until_arm_and_terminal_is_absorbing',
        'queued_gates_cannot_override_eof_or_early_stop',
        'real_startup_sequence_injects_faults_at_reachable_owned_operations',
        'real_startup_sequence_has_one_encoder_and_admits_only_after_park_and_writer',
        'feed_ack_precedes_finish_when_reader_pauses_before_stop_publication',
        'feed_end_never_acknowledges_startup_cancellation_or_invents_automatic_stop'])+'\ntest result: ok. 11 passed; 0 failed; 0 ignored;',
    'cursor-startup-contract': '\n'.join('test recorder::windows_capture::tests::'+name+' ... ok' for name in [
        'terminal_rejects_partial_killed_missing_and_inconsistent_results',
        'frozen_media_dimensions_stay_independent_of_observed_bounds_and_legacy_origin',
        'automatic_end_accepts_stop_during_finalization_without_inventing_an_ack'])+'\ntest result: ok. 11 passed; 0 failed; 0 ignored;',
    'cursor-startup-gate': '\n'.join('test recorder::owned_feed::tests::'+name+' ... ok' for name in [
        'startup_geometry_freezes_first_equal_callback_without_pixels',
        'startup_geometry_deadline_invalid_and_postfreeze_changes_fail',
        'startup_gate_action_is_not_upgraded_by_later_arm'])+'\ntest result: ok. 9 passed; 0 failed; 0 ignored;'}

class Gates(unittest.TestCase):
    def test_private_startup_unit_preflights_gate_profiles_and_keep_named_logs(self):
        # Missing, skipped or failed policy cases must prevent any owned source from starting.
        logs=STARTUP_UNIT_LOGS
        repo=Path(__file__).resolve().parents[2]
        metadata={'packages':[{'name':'windows-capture','source':None,'manifest_path':str(repo/'FocusFlow/vendor/windows-capture/Cargo.toml')}]}
        class ReadyForHarness(BaseException):pass
        for fault in ['valid','missing-lifecycle-name','missing-contract-name','missing-gate-name','failed-name','wrong-count','broad-22-log','ignored','command-failed']:
            with self.subTest(fault=fault),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);args=SimpleNamespace(evidence=root/'e',ffmpeg=root/'f',ffprobe=root/'p',scope='cursor-private');calls=[];builds=[]
                def execute(argv,evidence,name,*unused,**kwargs):
                    calls.append((name,argv));text=logs.get(name,'test result: ok. '+('6' if name=='cursor-portable' else '1')+' passed; 0 failed; 0 ignored;')
                    if name=='cursor-startup-lifecycle':
                        if fault=='missing-lifecycle-name':text=text.replace('queued_gates_cannot_override_eof_or_early_stop','other_case')
                        if fault=='failed-name':text=text.replace('feed_ack_precedes_finish_when_reader_pauses_before_stop_publication ... ok','feed_ack_precedes_finish_when_reader_pauses_before_stop_publication ... FAILED')
                        if fault=='wrong-count':text=text.replace('11 passed','10 passed')
                        if fault=='broad-22-log':text=text.replace('11 passed','22 passed')
                        if fault=='ignored':text=text.replace('0 ignored','1 ignored')
                        if fault=='command-failed':raise RuntimeError('unit command failed')
                    if name=='cursor-startup-gate' and fault=='missing-gate-name':text=text.replace('startup_geometry_freezes_first_equal_callback_without_pixels','other_case')
                    if name=='cursor-startup-contract' and fault=='missing-contract-name':text=text.replace('automatic_end_accepts_stop_during_finalization_without_inventing_an_ack','other_case')
                    path=evidence/(name+'.log');path.write_text(text);run.write(evidence/(name+'-command.json'),dict(exitCode=0));return path
                def build(*argv,**unused):builds.append(argv);raise ReadyForHarness
                with patch('run.sys.platform','win32'),patch.dict(run.os.environ,{'GITHUB_REF':'refs/heads/codex-windows-cursor-qualification'}),patch('run.require_cursor_disposable_context',return_value={'sha':'a'*40}),patch('run.os.chdir'),patch('run.source_manifest',return_value={}),patch('run.sha',side_effect=lambda p:run.FFMPEG_SHA if p==args.ffmpeg else run.FFPROBE_SHA if p==args.ffprobe else 'fixture'),patch('run.subprocess.check_output',side_effect=lambda argv,**kw:json.dumps(metadata) if argv[0]=='cargo' else 'a'*40),patch('run.execute',side_effect=execute),patch('run.build_exe',side_effect=build):
                    with self.assertRaises((ReadyForHarness,RuntimeError)):run.run(args)
                self.assertEqual([name for name,_ in calls],['cursor-renderer','cursor-portable','cursor-startup-lifecycle','cursor-startup-contract','cursor-startup-gate'])
                for (_,argv),filter_name in zip(calls[-3:],['recorder::windows_capture::lifecycle::tests::','recorder::windows_capture::tests::','recorder::owned_feed::tests::']):
                    self.assertEqual(argv,['cargo','+1.89.0','test','--locked','--manifest-path','FocusFlow/src-tauri/Cargo.toml','--lib',filter_name,'--','--test-threads=1','--nocapture'])
                self.assertEqual(len(builds),1 if fault=='valid' else 0)
                if fault!='valid':
                    result=json.loads((args.evidence/'result.json').read_text());self.assertFalse(result['cursorProofPassed']);self.assertFalse(result['passed']);self.assertFalse(result['productionCursorQualified'])
                else:
                    from package_evidence import package
                    kept=package(args.evidence,root/'absent',root/'upload')
                    self.assertTrue({name+suffix for name in logs for suffix in ['.log','-command.json']}<=set(kept))

    def test_native_source_manifest_retains_existing_licence(self):
        repo = Path(__file__).resolve().parents[2]
        self.assertEqual(run.source_manifest(repo)['FocusFlow/vendor/windows-capture/LICENCE'], 'e11eff681f6f0eb76dbea5787b100d646e4eb5d711e50cb5c9032595f6ca6a68')

    def test_windows_import_count_excludes_platform_only_tests(self):
        source = (Path(__file__).resolve().parents[2] / 'FocusFlow/crates/focusflow-core/tests/windows_capture.rs').read_text()
        tests = re.findall(r'((?:#\[[^\n]+\]\s*)+)fn (\w+)\(', source)
        windows = [name for attrs, name in tests if '#[test]' in attrs and '#[cfg(unix)]' not in attrs and '#[cfg(target_os="linux")]' not in attrs and '#[ignore' not in attrs]
        self.assertEqual(len(windows), 15)
        self.assertEqual(run.WINDOWS_IMPORT_TEST_COUNT, len(windows))
        run.require_test_count('test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;', run.WINDOWS_IMPORT_TEST_COUNT)
        for summary in ['17 passed; 0 failed; 0 ignored;', '14 passed; 0 failed; 1 ignored;', '15 passed; 0 failed; 1 ignored;']:
            with self.subTest(summary=summary), self.assertRaises(AssertionError):
                run.require_test_count('test result: ok. ' + summary, run.WINDOWS_IMPORT_TEST_COUNT)

    def test_import_command_skips_only_known_helper(self):
        repo=Path(__file__).resolve().parents[2]
        metadata={'packages':[{'name':'windows-capture','source':None,'manifest_path':str(repo/'FocusFlow/vendor/windows-capture/Cargo.toml')}]}
        class ImportReached(BaseException):pass
        def execute(argv,root,name,*args,**kwargs):
            if name=='core-import':
                self.assertEqual(argv[argv.index('--')+1:],['--test-threads=1','--nocapture','--skip','common::child_writer'])
                raise ImportReached
            log=root/(name+'.log');log.write_text('test result: ok. 10 passed; 0 failed; 0 ignored;');return log
        with tempfile.TemporaryDirectory() as tmp:
            args=SimpleNamespace(evidence=Path(tmp)/'evidence',ffmpeg=Path(tmp)/'ffmpeg',ffprobe=Path(tmp)/'ffprobe',scope='full')
            ambient_ref=run.os.getenv('GITHUB_REF')
            with patch.dict(run.os.environ,{'GITHUB_REF':'refs/heads/codex-windows-owned-product-qualification'}),patch('run.sys.platform','win32'),patch('run.os.chdir'),patch('run.source_manifest',return_value={}),patch('run.sha',side_effect=lambda p:run.FFMPEG_SHA if p==args.ffmpeg else run.FFPROBE_SHA),patch('run.subprocess.check_output',side_effect=lambda argv,**kw:json.dumps(metadata) if argv[0]=='cargo' else 'fixture-sha'),patch('run.execute',side_effect=execute):
                with self.assertRaises(ImportReached):run.run(args)
            self.assertEqual(run.os.getenv('GITHUB_REF'),ambient_ref)

    def test_exit7_requires_actual_exit_and_eof_without_other_failure(self):
        trace = dict(encoderExitCode=7, nonEncoderFailure=False, failure='Encoder exited 7', supervisorTerminated=None, overflow=False, selected=2, written=2, width=641, height=359, writes=[dict(ok=True, bytes=642*360*4)]*2)
        prefix = dict(consumerBytes=2*642*360*4, notWriterProgress=True, eof=True)
        with tempfile.TemporaryDirectory() as tmp:
            case = Path(tmp)
            def save(t=trace, p=prefix):
                (case/'trace.json').write_text(json.dumps(t))
                (case/'consumer-prefix.json').write_text(json.dumps(p))
            save(); run.require_exit7(case)
            for field, value in [('encoderExitCode', None), ('encoderExitCode', 1), ('nonEncoderFailure', True), ('failure', 'Output queue overflow/disconnected'), ('supervisorTerminated', True), ('supervisorTerminated', False), ('overflow', True), ('written', 1), ('writes', [dict(ok=False, bytes=1)])]:
                bad = copy.deepcopy(trace); bad[field] = value; save(bad)
                with self.subTest(field=field, value=value), self.assertRaises(AssertionError): run.require_exit7(case)
            bad = copy.deepcopy(trace); del bad['encoderExitCode']; save(bad)
            with self.assertRaises((AssertionError, KeyError)): run.require_exit7(case)
            for field, value in [('eof', False), ('consumerBytes', 0), ('consumerBytes', prefix['consumerBytes']-1)]:
                bad = dict(prefix); bad[field] = value; save(p=bad)
                with self.subTest(field=field), self.assertRaises(AssertionError): run.require_exit7(case)
            save(); (case/'consumer-prefix.json').unlink()
            with self.assertRaises(FileNotFoundError): run.require_exit7(case)


if __name__ == '__main__': unittest.main()


class ProductionStatic(unittest.TestCase):
    def fixture(self, tmp):
        root=Path(tmp)/'production-popup-even-r0';root.mkdir()
        source=root/'source';source.mkdir();session=root/'session-0';session.mkdir()
        run.fixture_write(source/'source-request.json',dict(width=320,height=240,nonce=700,animated=False,privateProfile='borderless'))
        run.fixture_write(source/'source-ready.json',dict(pid=22,hwnd=123))
        run.fixture_write(source/'source-observed.json',dict(pid=22,hwnd=123))
        run.fixture_write(source/'source-selection.json',dict(driverPid=11,sourcePid=22,hwnd=123))
        run.fixture_write(root/'production-source-ready.json',dict(hwnd=123,sourcePid=22))
        proc=SimpleNamespace(pid=22,poll=lambda:None)
        sources=SimpleNamespace(children={source:dict(proc=proc,observed=True,readyAt=10.)})
        return root,sources,{},{}

    def poll(self, root, sources, opened, state, now=10.):
        with patch('run.time.monotonic',return_value=now),patch('run.window_pid',return_value=22):
            run.poll_production_static(root,11,sources,opened,state)

    def arm(self,root,opened):
        owner=dict(parent=11,worker=33,encoder=44,qpc=3)
        run.fixture_write(root/'session-0/owned.json',owner)
        opened[root/'session-0/owned.json']=(owner,[SimpleNamespace(signaled=lambda:False),SimpleNamespace(signaled=lambda:False)])
        run.fixture_write(root/'production-armed.json',dict(hwnd=123,sourcePid=22,profile='pmv2_96_single_monitor_popup',candidateOnly=True))

    def raw(self,path,complete=True):
        path.write_bytes(b'P7\nWIDTH 320\nHEIGHT 240\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n'+b'\x60'*(320*240*4 if complete else 12))

    def test_hold_requires_two_full_raws_and_500ms(self):
        with tempfile.TemporaryDirectory() as tmp:
            root,sources,opened,state=self.fixture(tmp);self.poll(root,sources,opened,state)
            self.assertEqual(json.loads((root/'production-start-authorized.json').read_text()),dict(action='static-owned-canvas-v1',hwnd=123,sourcePid=22))
            self.arm(root,opened);self.poll(root,sources,opened,state,10.1)
            self.raw(root/'session-0/raw-0.pam');self.raw(root/'session-0/raw-1.pam',False)
            self.poll(root,sources,opened,state,10.7)
            self.assertFalse((root/'production-actions-complete.json').exists())
            self.raw(root/'session-0/raw-1.pam');self.poll(root,sources,opened,state,10.59)
            self.assertFalse((root/'production-actions-complete.json').exists())
            self.poll(root,sources,opened,state,10.7)
            self.assertEqual(json.loads((root/'production-actions-complete.json').read_text()),dict(action='static-hold-complete-v1',hwnd=123,sourcePid=22))

    def test_hold_timeout_and_armed_requires_observed_owner(self):
        for failure in ['timeout','unobserved-owner']:
            with self.subTest(failure=failure),tempfile.TemporaryDirectory() as tmp:
                root,sources,opened,state=self.fixture(tmp);self.poll(root,sources,opened,state);self.arm(root,opened)
                if failure=='unobserved-owner':opened.clear()
                else:self.poll(root,sources,opened,state,10.1)
                with self.assertRaises(AssertionError):self.poll(root,sources,opened,state,13.2 if failure=='timeout' else 10.1)
                self.assertFalse((root/'production-actions-complete.json').exists())

    def test_wrong_identity_profile_tuple_and_replaced_controls_refuse(self):
        for failure in ['pid','hwnd','profile','tuple','driver','extra','stale','replaced','symlink']:
            with self.subTest(failure=failure),tempfile.TemporaryDirectory() as tmp:
                root,sources,opened,state=self.fixture(tmp);p=root/'production-source-ready.json'
                if failure=='pid':run.write(p,dict(hwnd=123,sourcePid=99))
                if failure=='hwnd':run.write(p,dict(hwnd=999,sourcePid=22))
                if failure in ['profile','tuple']:
                    data=json.loads((root/'source/source-request.json').read_text());data['privateProfile' if failure=='profile' else 'nonce']='decorated' if failure=='profile' else 704
                    run.write(root/'source/source-request.json',data)
                if failure=='driver':run.write(root/'source/source-selection.json',dict(driverPid=99,sourcePid=22,hwnd=123))
                if failure=='extra':run.write(p,dict(hwnd=123,sourcePid=22,extra=True))
                if failure=='stale':run.write(root/'production-start-authorized.json',dict(action='static-owned-canvas-v1',hwnd=123,sourcePid=22))
                if failure=='replaced':
                    self.poll(root,sources,opened,state);run.write(root/'source/source-observed.json',dict(pid=22,hwnd=999))
                if failure=='symlink':
                    p.rename(root/'foreign.json');p.symlink_to(root/'foreign.json')
                with self.assertRaises(AssertionError):self.poll(root,sources,opened,state)

    def test_source_death_and_capture_deadline_fail_closed(self):
        for failure in ['death','deadline','premature-close']:
            with self.subTest(failure=failure),tempfile.TemporaryDirectory() as tmp:
                root,sources,opened,state=self.fixture(tmp);self.poll(root,sources,opened,state)
                if failure=='death':sources.children[root/'source']['proc'].poll=lambda:0
                if failure=='premature-close':run.fixture_write(root/'source/source-command-1.json',dict(sequence=1,action='close'))
                with self.assertRaises(AssertionError):self.poll(root,sources,opened,state,48.1 if failure=='deadline' else 10.1)

    def test_postcapture_requires_exact_join_and_normal_source_exit(self):
        with tempfile.TemporaryDirectory() as tmp:
            root,sources,opened,state=self.fixture(tmp);self.poll(root,sources,opened,state);self.arm(root,opened);self.poll(root,sources,opened,state,10.1)
            for i in [0,1]:self.raw(root/f'session-0/raw-{i}.pam')
            self.poll(root,sources,opened,state,10.7)
            run.fixture_write(root/'production-phase.json',dict(phase='postcapture',hwnd=123,sourcePid=22,workerEncoderJoined=True,sourceClosed=True,qpc=123))
            run.fixture_write(root/'source/source-command-1.json',dict(sequence=1,action='close'))
            sources.children[root/'source']['proc'].poll=lambda:0
            run.fixture_write(root/'source/source-exit.json',dict(pid=22,exitCode=0))
            with self.assertRaises(AssertionError):self.poll(root,sources,opened,state,11.)
            for handle in opened[root/'session-0/owned.json'][1]:handle.signaled=lambda:True
            self.poll(root,sources,opened,state,11.)
            self.assertEqual(state['phase'],'postcapture')


class ProductionRouting(unittest.TestCase):
    def test_static_scope_runs_only_eight_actual_cases_and_aborts_unknown_cleanup(self):
        repo=Path(__file__).resolve().parents[2];metadata={'packages':[{'name':'windows-capture','source':None,'manifest_path':str(repo/'FocusFlow/vendor/windows-capture/Cargo.toml')}]}
        for failure in [False,True]:
            with self.subTest(failure=failure),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);args=SimpleNamespace(evidence=root/'e',ffmpeg=root/'f',ffprobe=root/'p',scope='cursor-production-static');calls=[]
                def execute(argv,evidence,name,*rest,**kw):
                    calls.append((name,list(argv),rest,kw));self.assertEqual(argv[2],'cursor-production');self.assertEqual(kw.get('production_static'),True)
                    case=evidence/name;case.mkdir();log=evidence/(name+'.log');log.touch()
                    if failure:raise RuntimeError('unknown child cleanup')
                    return log
                import production_cursor_check
                with patch('run.sys.platform','win32'),patch.dict(run.os.environ,{'GITHUB_REF':'refs/heads/codex-windows-cursor-qualification'}),patch('run.require_cursor_disposable_context',return_value={'sha':'a'*40}),patch('run.os.chdir'),patch('run.source_manifest',return_value={}),patch('run.sha',side_effect=lambda p:run.FFMPEG_SHA if p==args.ffmpeg else run.FFPROBE_SHA if p==args.ffprobe else 'fixture'),patch('run.subprocess.check_output',side_effect=lambda argv,**kw:json.dumps(metadata) if argv[0]=='cargo' else 'a'*40),patch('run.execute',side_effect=execute),patch('run.build_exe',return_value=root/'harness'),patch('run.require_production_cleanup'),patch.object(production_cursor_check,'check',return_value=dict(productionStaticCasesPassed=True,productionCursorQualified=False)):
                    if failure:
                        with self.assertRaises(RuntimeError):run.run(args)
                    else:run.run(args)
                self.assertEqual(len(calls),1 if failure else 8)
                self.assertEqual([row[0] for row in calls],['production-popup-even-r0'] if failure else ['production-popup-even-r0','production-decorated-even-r0','production-popup-odd-r0','production-decorated-odd-r0','production-popup-even-r1','production-decorated-even-r1','production-popup-odd-r1','production-decorated-odd-r1'])
                result=json.loads((args.evidence/'result.json').read_text());self.assertFalse(result['passed']);self.assertFalse(result['productionCursorQualified']);self.assertEqual(result['productionStaticCasesPassed'],not failure)

    def test_live_caps_and_only_named_production_pending_may_disappear(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);source=root/'production-popup-even-r0/source';source.mkdir(parents=True)
            control=source.parent/'production-placement.json';control.write_bytes(b' '*4097)
            with self.assertRaises(AssertionError):run.bound(root)
            control.unlink();pending=source/'source-observed.pending';pending.touch();original=Path.stat
            def stat(path,*a,**kw):
                if path==pending:raise FileNotFoundError(path)
                return original(path,*a,**kw)
            with patch.object(Path,'stat',stat):run.bound(root)
            pending.rename(source/'foreign.pending');pending=source/'foreign.pending'
            with patch.object(Path,'stat',stat),self.assertRaises(FileNotFoundError):run.bound(root)


class ProductionIntegrity(unittest.TestCase):
    fixture=ProductionStatic.fixture
    poll=ProductionStatic.poll
    arm=ProductionStatic.arm
    def test_same_bytes_replaced_control_and_malformed_authorization_refuse(self):
        for failure in ['replace-same-bytes','float-pid','duplicate-key','extra-key']:
            with self.subTest(failure=failure),tempfile.TemporaryDirectory() as tmp:
                root,sources,opened,state=self.fixture(tmp);self.poll(root,sources,opened,state)
                p=root/'production-start-authorized.json'
                if failure=='replace-same-bytes':
                    fresh=p.with_name('replacement');fresh.write_bytes(p.read_bytes());fresh.replace(p)
                elif failure=='float-pid':p.write_text('{"action":"static-owned-canvas-v1","hwnd":123,"sourcePid":22.0}')
                elif failure=='duplicate-key':p.write_text('{"action":"static-owned-canvas-v1","hwnd":123,"hwnd":123,"sourcePid":22}')
                else:p.write_text('{"action":"static-owned-canvas-v1","hwnd":123,"sourcePid":22,"extra":true}')
                with self.assertRaises(AssertionError):self.poll(root,sources,opened,state)

    def test_cleanup_requires_normal_exact_owners_and_rejects_unknown(self):
        import copy
        with tempfile.TemporaryDirectory() as tmp:
            root,sources,opened,state=self.fixture(tmp);self.arm(root,opened)
            owner=dict(parent=11,worker=33,encoder=44,qpc=3);source=dict(pid=22,observed=True,signaled=True,exitCode=0)
            saved=dict(**source,fallback=False,error=None);run.fixture_write(root/'source/source-cleanup.json',saved)
            receipt=root.parent/'command.json';data=dict(error=None,exitCode=0,cleanupUnknown=False,cleanup=dict(parent=dict(pid=11,signaled=True,exitCode=0),owned=[dict(**owner,path='session-0/owned.json',signaled=[True,True])],sources=[source]),ownership=[dict(**owner,signaled=[True,True],fallback=False)],productionStaticPhase=dict(phase='postcapture'))
            run.write(receipt,data);run.require_production_cleanup(root,receipt,complete=True)
            for failure in ['unknown','fallback','source-exit','unobserved','pair-missing','wrong-parent']:
                bad=copy.deepcopy(data)
                if failure=='unknown':bad['cleanupUnknown']=True
                if failure=='fallback':bad['ownership'][0]['fallback']=True
                if failure=='source-exit':bad['cleanup']['sources'][0]['exitCode']=7
                if failure=='unobserved':bad['cleanup']['sources'][0]['observed']=False
                if failure=='pair-missing':bad['cleanup']['owned']=[]
                if failure=='wrong-parent':bad['cleanup']['owned'][0]['parent']=999
                run.write(receipt,bad)
                with self.subTest(failure=failure),self.assertRaises(AssertionError):run.require_production_cleanup(root,receipt,complete=True)


class ProductionSnapshotFix(unittest.TestCase):
    fixture=ProductionStatic.fixture
    poll=ProductionStatic.poll
    def test_replacement_between_validation_and_remember_never_authorizes(self):
        for same_bytes in [False,True]:
            with self.subTest(same_bytes=same_bytes),tempfile.TemporaryDirectory() as tmp:
                root,sources,opened,state=self.fixture(tmp);marker=root/'production-source-ready.json';read=run.production_read;replaced=[]
                def replace_after_validated(path,*args,**kwargs):
                    result=read(path,*args,**kwargs)
                    if path==marker and not replaced:
                        fresh=marker.with_name('replacement');fresh.write_bytes(marker.read_bytes() if same_bytes else b'{"hwnd":999,"sourcePid":22}');fresh.replace(marker);replaced.append(True)
                    return result
                with patch('run.production_read',side_effect=replace_after_validated):
                    with self.assertRaises(AssertionError):self.poll(root,sources,opened,state)
                self.assertTrue(replaced);self.assertFalse((root/'production-start-authorized.json').exists())

    def test_owned_retry_only_accepts_incomplete_json_prefixes(self):
        for text,incomplete in [('',True),('  ',True),('{"encoder":',True),('{"enc',True),('{"encoder":33,',True),('{"encoder":33,}',False),('{"encoder":33}trailing',False),('not-json',False)]:
            with self.subTest(text=text):
                try:json.loads(text)
                except json.JSONDecodeError as error:self.assertEqual(run.incomplete_owned_json(error),incomplete)
                else:self.fail('fixture must be invalid JSON')


    def test_incomplete_owned_identity_and_two_second_deadline_stay_strict(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);path=root/'owned.json';path.write_bytes(b'{"encoder":');state=dict(initialOwnedAt=10.)
            with patch('run.time.monotonic',return_value=10.):
                try:run.production_read(path,root)
                except json.JSONDecodeError as error:run.initial_owned_snapshot(state,error.production_snapshot)
            fresh=root/'replacement';fresh.write_bytes(path.read_bytes());fresh.replace(path)
            with patch('run.time.monotonic',return_value=10.1):
                try:run.production_read(path,root)
                except json.JSONDecodeError as error:
                    with self.assertRaises(AssertionError):run.initial_owned_snapshot(state,error.production_snapshot)
            state=dict(initialOwnedAt=10.)
            with patch('run.time.monotonic',return_value=12.):
                try:run.production_read(path,root)
                except json.JSONDecodeError as error:
                    with self.assertRaisesRegex(AssertionError,'publication deadline'):run.initial_owned_snapshot(state,error.production_snapshot)

    def test_replacement_inside_validation_never_adopts_new_file_snapshot(self):
        for same_bytes in [False,True]:
            with self.subTest(same_bytes=same_bytes),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);path=root/'control.json';path.write_text('{"hwnd":123,"sourcePid":22}');loads=run.json.loads;replaced=[];denied=[];cache={}
                original=path.read_bytes();identity=run.production_stat(path.stat());fresh=root/'replacement'
                def validating(*args,**kwargs):
                    value=loads(*args,**kwargs);fresh.write_bytes(original if same_bytes else b'{"hwnd":999,"sourcePid":22}')
                    try:fresh.replace(path)
                    except PermissionError as error:
                        denied.append(error);raise
                    replaced.append(True);return value
                with patch('run.json.loads',side_effect=validating),patch('run.production_remember',wraps=run.production_remember) as remember:
                    with self.assertRaises((AssertionError,PermissionError)) as rejected:run.production_control(path,root,cache)
                    remember.assert_not_called()
                if isinstance(rejected.exception,PermissionError):
                    # Windows can deny replacing this still-open file. Only that exact denial counts.
                    self.assertEqual(denied,[rejected.exception]);self.assertEqual(getattr(rejected.exception,'winerror',None),5)
                    self.assertEqual(Path(rejected.exception.filename),fresh);self.assertEqual(Path(rejected.exception.filename2),path)
                    self.assertFalse(replaced);self.assertEqual(path.read_bytes(),original);self.assertEqual(run.production_stat(path.stat()),identity)
                else:self.assertTrue(replaced);self.assertFalse(denied)
                self.assertEqual(cache,{})
                self.assertFalse((root/'production-start-authorized.json').exists());self.assertFalse((root/'handles-open').exists())


    def test_nonregular_and_oversized_controls_are_rejected_before_open(self):
        for kind in ['directory','oversized']:
            with self.subTest(kind=kind),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);path=root/'control.json'
                if kind=='directory':path.mkdir()
                else:path.write_bytes(b' '*4097)
                with patch.object(Path,'open',side_effect=AssertionError('invalid control must not be opened')) as opened:
                    with self.assertRaises(AssertionError):run.production_read(path,root)
                    opened.assert_not_called()

class ProductionFinalRouting(unittest.TestCase):
    def test_existing_workflow_selects_only_static_cursor_branch_and_uploads_bound_assets(self):
        import fnmatch,re
        workflow=(Path(__file__).resolve().parents[2]/'.github/workflows/windows-owned-product-qualification.yml').read_text()
        expression=re.search(r'FOCUSFLOW_QUAL_SCOPE: \$\{\{ (.*?) \}\}',workflow)[1]
        routes=re.findall(r"github.ref == '([^']+)' && '([^']+)'",expression)
        for branch,expected in [('codex-windows-cursor-qualification','cursor-production-static'),('codex-windows-startup-metadata','startup-metadata'),('codex-windows-owned-close-diagnostic','close-diagnostic'),('codex-windows-owned-product-qualification','full')]:
            selected=next((scope for ref,scope in routes if ref=='refs/heads/'+branch),'full');self.assertEqual(selected,expected)
        patterns=[line.strip() for line in workflow.splitlines() if line.strip().startswith('qualification-upload/')]
        for name in ['session-0/capture.windows-v3','session-0/cursor/observations.ndjson','session-0/cursor/input-observations.ndjson','session-0/cursor/shapes/shape-0.pam']:
            self.assertTrue(any(fnmatch.fnmatchcase('qualification-upload/cases/production-popup-even-r0/'+name,p) for p in patterns),name)

    def test_only_owned_atomic_temps_live_and_final_evidence_forbids_leftovers(self):
        import package_evidence
        for name in ['clicks.json.tmp','drags.json.tmp','timeline.json.tmp','.capture.json-11-0.tmp','.project.json-11-18446744073709551615.tmp','.capture.json-99-0.tmp','.capture.json-11-00.tmp','.capture.json-11-18446744073709551616.tmp','other.json.tmp']:
            with self.subTest(name=name),tempfile.TemporaryDirectory() as d:
                root=Path(d);case=root/'production-popup-even-r0';session=case/'session-0';session.mkdir(parents=True);path=session/name;path.write_bytes(b'{}')
                accepted=name in {'clicks.json.tmp','drags.json.tmp','timeline.json.tmp','.capture.json-11-0.tmp','.project.json-11-18446744073709551615.tmp'}
                if accepted:
                    run.bound(root,production_parent=(case.name,11))
                    with self.assertRaises(AssertionError):run.bound(root)
                    original=Path.stat
                    def disappearing(p,*args,**kwargs):
                        if p==path:raise FileNotFoundError(p)
                        return original(p,*args,**kwargs)
                    with patch.object(Path,'stat',disappearing):run.bound(root,production_parent=(case.name,11))
                else:
                    with self.assertRaises(AssertionError):run.bound(root,production_parent=(case.name,11))

class ProductionProviderStability(unittest.TestCase):
    def test_distinct_stable_provider_ctimes_keep_single_handle_bytes(self):
        # Linux cannot produce Windows' creation-time/path vs change-time/handle split.
        # Only fstat metadata is adapted; path checks, one open/read, JSON and digest are real.
        import hashlib,os
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);path=root/'control.json';raw=b'{"value":7}';path.write_bytes(raw)
            original=os.fstat;opens=Path.open;opened=[]
            def handle_stat(fd):
                info=original(fd)
                return SimpleNamespace(**{key:getattr(info,key) for key in ['st_dev','st_ino','st_mode','st_size','st_mtime_ns']},st_ctime_ns=info.st_ctime_ns+100)
            def opening(candidate,*args,**kwargs):
                if candidate==path:opened.append(candidate)
                return opens(candidate,*args,**kwargs)
            with patch('run.os.fstat',side_effect=handle_stat),patch.object(Path,'open',opening):
                value,snapshot=run.production_read(path,root)
                self.assertEqual(value,{'value':7});self.assertEqual(snapshot[3],hashlib.sha256(raw).hexdigest())
                self.assertEqual(run.production_remember(path,root,value,snapshot,{}),value)
                with patch('run.time.monotonic',return_value=10.):
                    self.assertTrue(run.initial_owned_snapshot({'initialOwnedAt':10.},snapshot))
            self.assertEqual(len(opened),1)

    def test_partial_owned_uses_same_provider_stability_without_authorizing(self):
        import os
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);path=root/'owned.json';path.write_bytes(b'{"encoder":');original=os.fstat
            def handle_stat(fd):
                info=original(fd)
                return SimpleNamespace(**{key:getattr(info,key) for key in ['st_dev','st_ino','st_mode','st_size','st_mtime_ns']},st_ctime_ns=info.st_ctime_ns+100)
            with patch('run.os.fstat',side_effect=handle_stat),patch('run.time.monotonic',return_value=10.):
                with self.assertRaises(json.JSONDecodeError) as caught:run.production_read(path,root)
                self.assertTrue(run.incomplete_owned_json(caught.exception))
                self.assertTrue(run.initial_owned_snapshot({'initialOwnedAt':10.},caught.exception.production_snapshot))
            self.assertFalse((root/'handles-open').exists())

    def test_each_provider_change_and_cross_provider_join_stays_strict(self):
        # Alter only the metadata observation boundary; the actual bounded file read stays real.
        for observation,field in [(0,5),(1,5),(2,5),(3,5),(4,5),(2,3),(3,3),(2,4),(3,4),(2,1),(3,1)]:
            with self.subTest(observation=observation,field=field),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);path=root/'control.json';path.write_bytes(b'{"value":7}');base=run.production_stat(path.stat())
                observations=[list(base) for _ in range(5)];observations[observation][field]+=1
                with patch('run.production_stat',side_effect=[tuple(x) for x in observations]):
                    with self.assertRaises(AssertionError):run.production_control(path,root,{})
        for field in [3,4]:
            with self.subTest(cross_provider_field=field),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);path=root/'control.json';path.write_bytes(b'{"value":7}');base=run.production_stat(path.stat())
                handle=list(base);handle[field]+=1
                with patch('run.production_stat',side_effect=[base,tuple(handle),tuple(handle),base,base]):
                    with self.assertRaises(AssertionError):run.production_control(path,root,{})

    def test_stable_provider_ctime_difference_does_not_bypass_byte_count(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);path=root/'control.json';path.write_bytes(b'{"value":7}');base=list(run.production_stat(path.stat()));base[3]+=1
            handle=base.copy();handle[5]+=100
            with patch('run.production_stat',side_effect=[tuple(base),tuple(handle),tuple(handle),tuple(base)]):
                with self.assertRaisesRegex(AssertionError,'byte count'):run.production_read(path,root)

    def test_cached_digest_and_each_provider_ctime_are_immutable(self):
        for change in ['bytes','path-ctime','handle-ctime']:
            with self.subTest(change=change),tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp);path=root/'control.json';path.write_bytes(b'{"value":7}');base=run.production_stat(path.stat());cache={}
                with patch('run.production_stat',return_value=base):run.production_control(path,root,cache)
                path_meta=list(base);handle=list(base)
                if change=='bytes':path.write_bytes(b'{"value":8}')
                elif change=='path-ctime':path_meta[5]+=1
                else:handle[5]+=1
                observations=[path_meta,handle,handle,path_meta,path_meta]
                with patch('run.production_stat',side_effect=[tuple(x) for x in observations]):
                    with self.assertRaisesRegex(AssertionError,'production control replaced'):run.production_control(path,root,cache)
