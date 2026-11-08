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
            args=SimpleNamespace(evidence=Path(tmp)/'evidence',ffmpeg=Path(tmp)/'ffmpeg',ffprobe=Path(tmp)/'ffprobe')
            with patch('run.sys.platform','win32'),patch('run.os.chdir'),patch('run.source_manifest',return_value={}),patch('run.sha',side_effect=lambda p:run.FFMPEG_SHA if p==args.ffmpeg else run.FFPROBE_SHA),patch('run.subprocess.check_output',side_effect=lambda argv,**kw:json.dumps(metadata) if argv[0]=='cargo' else 'fixture-sha'),patch('run.execute',side_effect=execute):
                with self.assertRaises(ImportReached):run.run(args)

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
