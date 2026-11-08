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

class ProductionEvidence(unittest.TestCase):
    def fixture(self,root):
        from test_production_cursor_check import synthetic_case
        ev=root/'evidence';ev.mkdir();case=synthetic_case(ev)
        return ev,case

    def test_production_exact_assets_v3_and_streams_packaged(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);ev,case=self.fixture(root)
            hashes=package(ev,root/'absent',root/'upload');prefix='cases/'+case.name+'/'
            for name in ['production-geometry.json','production-placement.json','source/source-ready.json','session-0/capture.windows-v3','session-0/cursor/observations.ndjson','session-0/cursor/input-observations.ndjson','session-0/cursor/shapes/shape-0.pam','session-0/raw-1.pam','native-raster-oracle/raster-0-3.pam']:
                self.assertIn(prefix+name,hashes)
                self.assertEqual((root/'upload'/prefix/name).read_bytes(),(case/name).read_bytes())

    def test_shape_traversal_absolute_links_unknown_files_and_omission_fail_without_deleting(self):
        for bad in ['../secret.pam','/secret.pam','C:/secret.pam','cursor/shapes/shape-1.pam','link','extra','missing']:
            with self.subTest(bad=bad),tempfile.TemporaryDirectory() as d:
                root=Path(d);ev,case=self.fixture(root);index=case/'session-0/cursor/index.json'
                original=(case/'session-0/raw-0.pam').read_bytes()
                if bad=='link':
                    shape=case/'session-0/cursor/shapes/shape-0.pam';data=shape.read_bytes();shape.unlink();(root/'foreign.pam').write_bytes(data);shape.symlink_to(root/'foreign.pam')
                elif bad=='extra':(case/'session-0/cursor/shapes/secret.pam').write_bytes(b'secret')
                elif bad=='missing':(case/'session-0/cursor/shapes/shape-0.pam').unlink()
                else:
                    data=json.loads(index.read_text());data['shapes'][0]['path']=bad;index.write_text(json.dumps(data))
                with self.assertRaises((RuntimeError,AssertionError,FileNotFoundError)):package(ev,root/'absent',root/'upload')
                self.assertEqual((case/'session-0/raw-0.pam').read_bytes(),original)

    def test_production_caps_apply_at_collection_and_package(self):
        for name,size in [('production-placement.json',4097),('session-0/cursor/index.json',65537),('production-sampled-interactions.json',8*1024*1024+1),('session-0/cursor/observations.ndjson',2049)]:
            with self.subTest(name=name),tempfile.TemporaryDirectory() as d:
                root=Path(d);ev,case=self.fixture(root)
                with (case/name).open('wb') as f:f.truncate(size)
                with self.assertRaises(AssertionError):bound(ev)
                with self.assertRaises((RuntimeError,AssertionError,ValueError)):package(ev,root/'absent',root/'upload')
                self.assertEqual((case/name).stat().st_size,size)

    def test_unknown_static_file_and_extra_shape_fail_live_collection(self):
        for name in ['session-0/user.json','session-0/cursor/shapes/shape-1.pam','source/source-command-2.json','arbitrary.pending']:
            with self.subTest(name=name),tempfile.TemporaryDirectory() as d:
                root=Path(d);ev,case=self.fixture(root);(case/name).write_bytes(b'{}')
                with self.assertRaises(AssertionError):bound(ev)

    def test_empty_import_lock_is_omitted_and_nonempty_or_foreign_locks_rejected(self):
        for name,data in [('session-0.document',b''),('session-0.document',b'x'),('foreign.document',b'')]:
            with self.subTest(name=name,data=data),tempfile.TemporaryDirectory() as d:
                root=Path(d);ev,case=self.fixture(root);locks=case/'.locks';locks.mkdir();(locks/name).write_bytes(data)
                if name=='session-0.document' and not data:
                    bound(ev);hashes=package(ev,root/'absent',root/'upload');self.assertFalse(any('.locks' in key for key in hashes))
                else:
                    with self.assertRaises(AssertionError):bound(ev)
                    with self.assertRaises(RuntimeError):package(ev,root/'absent',root/'upload')

    def test_aggregate_cap_and_success_omission_preserve_local_originals(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);ev=root/'evidence';ev.mkdir()
            for name in ['production-popup-even-r0','production-popup-even-r1','production-decorated-even-r0','production-decorated-even-r1']:
                case=ev/name/'session-0';case.mkdir(parents=True)
                with (case/'screen.mp4').open('wb') as f:f.truncate(64*1024*1024)
            (ev/'result.json').write_text('{}')
            with self.assertRaises(AssertionError):bound(ev)
            with patch('package_evidence.shutil.copyfile',side_effect=lambda p,t:t.touch()),self.assertRaises(RuntimeError):package(ev,root/'absent',root/'upload')
            report=json.loads((root/'upload/upload-manifest.json').read_text());self.assertTrue(report['capFailure']);self.assertLessEqual(report['bytes'],255*1024*1024)
            self.assertEqual(len(list(ev.glob('*/session-0/screen.mp4'))),4)
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);ev=root/'evidence';ev.mkdir();(ev/'result.json').write_text(json.dumps(dict(scope='cursor-production-static',productionStaticCasesPassed=True)))
            with self.assertRaises(RuntimeError):package(ev,root/'absent',root/'upload')

    def test_private_package_rejects_public_v2_and_v3_authority(self):
        for name in ['capture.json','capture.windows-v2','capture.windows-v3','project.json','screen.mp4']:
            with self.subTest(name=name),tempfile.TemporaryDirectory() as d:
                root=Path(d);case=root/'evidence/cursor-borderless-even/session-0';case.mkdir(parents=True);(case/name).write_bytes(b'public authority')
                with self.assertRaises(RuntimeError):package(root/'evidence',root/'absent',root/'upload')
                self.assertTrue((case/name).exists())
