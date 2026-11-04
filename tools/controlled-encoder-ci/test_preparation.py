import importlib.util
import io
import json
import pathlib
import tempfile
import tarfile
import unittest
from unittest.mock import patch

import prepare
import acquire
import hashlib
import sys
import artifact
import validate


class PreparationControls(unittest.TestCase):
    def test_original_recipe_and_exact_inputs(self):
        lock = validate.validate()
        self.assertEqual(len([x for x in lock['files'] if x['kind'] == 'encoder']), 3)
        self.assertEqual(len([x for x in lock['files'] if x['kind'] == 'mingw-rpm']), 11)
        self.assertEqual(len([x for x in lock['files'] if x['kind'] == 'source-rpm']), 7)

    def test_stale_unsigned_size_rejected_against_signed_source_receipt(self):
        lock = json.loads((validate.BASE / 'inputs.lock.json').read_text())
        receipts = json.loads((validate.BASE / 'provenance/source-rpm-acquisition.json').read_text())
        row = next(row for row in lock['files'] if row['name'] == 'mingw-filesystem-152-1.fc44.src.rpm')
        row['bytes'] = 91490
        with self.assertRaisesRegex(ValueError, 'mingw-filesystem.*bytes'):
            validate.validate_source_receipts(lock, receipts)

    def test_bounded_download_persists_observed_size_and_rejects_extra_bytes(self):
        for exact in [91490,97250]:
            with self.subTest(exact=exact), tempfile.TemporaryDirectory() as tmp:
                root=pathlib.Path(tmp);cache=root/'cache'
                data=b'x'*97250
                row={'name':'signed-source-fixture.rpm','url':'https://example.invalid/source.rpm','bytes':exact,'sha256':hashlib.sha256(data).hexdigest()}
                (root/'inputs.lock.json').write_text(json.dumps({'files':[row]}))
                response=io.BytesIO(data);response.url=row['url']
                with patch.object(acquire,'BASE',root),patch('urllib.request.urlopen',return_value=response),patch.object(sys,'argv',['acquire.py','--cache',str(cache)]):
                    if exact==91490:
                        with self.assertRaisesRegex(ValueError, 'signed-source-fixture.rpm.*97250.*91490'):
                            acquire.main()
                        failure=json.loads((cache/'acquisition-failure.json').read_text())
                        self.assertEqual(failure['received_bytes'],97250)
                        self.assertEqual(failure['exact_bytes'],91490)
                        self.assertEqual(failure['name'],row['name'])
                    else:
                        acquire.main()
                        self.assertFalse((cache/'acquisition-failure.json').exists())
                        self.assertEqual((cache/row['name']).read_bytes(),data)

    def test_missing_platform_pin_blocks_publication(self):
        lock = json.loads((validate.BASE / 'inputs.lock.json').read_text())
        lock['platform']['host_helper_policy'] = None
        original = pathlib.Path.read_text
        def read(path, *a, **k):
            return json.dumps(lock) if path.name == 'inputs.lock.json' else original(path, *a, **k)
        with patch.object(pathlib.Path, 'read_text', read), self.assertRaisesRegex(ValueError, 'host helper policy'):
            validate.validate(platform=True)

    def test_host_gate_rejects_missing_native_compiler(self):
        for missing in ['gcc', None]:
            with self.subTest(missing=missing), patch.object(sys, 'argv', ['validate.py', '--host']), \
                    patch('shutil.which', side_effect=lambda tool: None if tool == missing else '/usr/bin/' + tool), \
                    patch('sys.stdout', new_callable=io.StringIO):
                if missing:
                    with self.assertRaisesRegex(ValueError, 'Required host tool missing: gcc'):
                        validate.main()
                else:
                    validate.main()

    def test_tar_rejects_traversal_and_escaping_link(self):
        for name, target in [('../escape', None), ('safe/link', '/tmp/escape')]:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as tmp:
                root = pathlib.Path(tmp)
                archive = root / 'source.tar'
                with tarfile.open(archive, 'w') as out:
                    member = tarfile.TarInfo(name)
                    if target:
                        member.type = tarfile.SYMTYPE
                        member.linkname = target
                        out.addfile(member)
                    else:
                        member.size = 1
                        out.addfile(member, io.BytesIO(b'x'))
                with self.assertRaises((ValueError, tarfile.FilterError)):
                    prepare.safe_tar(archive, root / 'owned')
                self.assertFalse((root / 'escape').exists())

    def test_header_rejects_symlink_parent_before_cpio(self):
        header = '/usr/link 1 0 0 0120777 root root 0 0 0 /tmp\n/usr/link/file 1 0 0 0100644 root root 0 0 0 X\n'
        with patch('subprocess.check_output', return_value=header), self.assertRaisesRegex(ValueError, 'symlink parent'):
            prepare.rpm_entries(pathlib.Path('synthetic.rpm'))

    def test_artifact_rejects_tampered_executable(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root/'candidate/receipts').mkdir(parents=True)
            pe=root/'candidate/ffmpeg.exe';pe.write_bytes(b'synthetic PE fixture')
            receipt=root/'candidate/receipts/candidate.json'
            receipt.write_text(json.dumps({'sha256':prepare.sha(pe)}))
            manifest=[{'path':str(path.relative_to(root)), 'bytes':path.stat().st_size, 'sha256':prepare.sha(path)} for path in [pe,receipt]]
            (root/'manifest.json').write_text(json.dumps(manifest))
            artifact.verify(root)
            pe.write_bytes(b'tampered')
            with self.assertRaisesRegex(ValueError, 'Artifact hash mismatch'):
                artifact.verify(root)

    def test_output_paths_are_explicit_and_verifier_only_decodes(self):
        smoke = (prepare.BASE / 'windows-smoke.py').read_text()
        self.assertNotIn('/usr/bin/ff', smoke)
        self.assertNotIn('returncode<0', smoke)
        self.assertIn('Candidate must be distinct', smoke)
        self.assertIn("exe=args.ffmpeg_reference", smoke)
        self.assertNotIn("'libx264'", '\n'.join(line for line in smoke.splitlines() if 'exe=args.ffmpeg_reference' in line))


if __name__ == '__main__':
    unittest.main()
