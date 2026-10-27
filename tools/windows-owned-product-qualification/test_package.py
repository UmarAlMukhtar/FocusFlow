import json
from pathlib import Path, PureWindowsPath
import tempfile
import unittest
from unittest.mock import patch
from package_evidence import package
from run import bound

class Evidence(unittest.TestCase):
    def test_windows_relative_paths_are_canonical_in_success_and_cap_manifest(self):
        with tempfile.TemporaryDirectory() as t:
            root=Path(t);ev=root/'evidence';case=ev/'geometry-320x240/session-0';case.mkdir(parents=True)
            (case/'capture.json').write_text('{}')
            expected='cases/geometry-320x240/session-0/capture.json'
            with patch('package_evidence.Path',PureWindowsPath):
                hashes=package(ev,root/'absent',root/'success')
            self.assertEqual(set(hashes),{expected})
            manifest=json.loads((root/'success/upload-manifest.json').read_text())
            self.assertEqual(manifest['sha256'],hashes)
            self.assertEqual((root/'success'/expected).read_bytes(),b'{}')
            with (case/'trace.json').open('wb') as f:f.truncate(4*1024*1024+1)
            with patch('package_evidence.Path',PureWindowsPath), self.assertRaises(RuntimeError):
                package(ev,root/'absent',root/'capped')
            manifest=json.loads((root/'capped/upload-manifest.json').read_text())
            self.assertEqual(set(manifest['sha256']),{expected})
            self.assertEqual(manifest['omitted'],['cases/geometry-320x240/session-0/trace.json'])
            self.assertTrue(manifest['capFailure'])
            self.assertFalse((root/'capped/cases/geometry-320x240/session-0/trace.json').exists())

    def test_only_named_generated_files_are_uploaded(self):
        with tempfile.TemporaryDirectory() as t:
            root=Path(t);ev=root/'evidence';ev.mkdir();setup=root/'setup';setup.mkdir()
            (ev/'private.mp4').write_bytes(b'not allowed');(ev/'result.json').write_text('{}')
            case=ev/'normal/session-0';case.mkdir(parents=True);(case/'screen.mp4').write_bytes(b'wrong case')
            good=ev/'geometry-320x240/session-0';good.mkdir(parents=True);(good/'capture.json').write_text('{}');(good/'user-secret.json').write_text('{}')
            hashes=package(ev,setup,root/'upload')
            self.assertEqual(set(hashes),{'result.json','cases/geometry-320x240/session-0/capture.json'})
    def test_oversized_trace_fails_without_copying_and_retains_cap_receipt(self):
        with tempfile.TemporaryDirectory() as t:
            root=Path(t);ev=root/'evidence';case=ev/'geometry-320x240/session-0';case.mkdir(parents=True)
            with (case/'trace.json').open('wb') as f:f.truncate(4*1024*1024+1)
            with self.assertRaises(AssertionError):bound(ev)
            with self.assertRaises(RuntimeError):package(ev,root/'absent',root/'upload')
            receipt=json.loads((root/'upload/upload-manifest.json').read_text());self.assertTrue(receipt['capFailure']);self.assertFalse((root/'upload/cases/geometry-320x240/session-0/trace.json').exists())
    def test_root_command_logs_allowlisted(self):
        with tempfile.TemporaryDirectory() as t:
            root=Path(t);ev=root/'e';ev.mkdir();(ev/'blocked.log').write_text('failure');(ev/'unrelated.log').write_text('private')
            self.assertEqual(set(package(ev,root/'absent',root/'upload')),{'blocked.log'})
if __name__=='__main__':unittest.main()
