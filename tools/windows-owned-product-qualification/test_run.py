import copy
import json
from pathlib import Path
import re
import tempfile
import unittest
import run


class Gates(unittest.TestCase):
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
