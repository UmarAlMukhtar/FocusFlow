import importlib.util
import io
import json
import pathlib
import tempfile
import tarfile
import unittest
from unittest.mock import patch

import prepare
import artifact
import validate


class PreparationControls(unittest.TestCase):
    def test_original_recipe_and_exact_inputs(self):
        lock = validate.validate()
        self.assertEqual(len([x for x in lock['files'] if x['kind'] == 'encoder']), 3)
        self.assertEqual(len([x for x in lock['files'] if x['kind'] == 'mingw-rpm']), 11)
        self.assertEqual(len([x for x in lock['files'] if x['kind'] == 'source-rpm']), 7)

    def test_missing_platform_pin_blocks_publication(self):
        lock = json.loads((validate.BASE / 'inputs.lock.json').read_text())
        lock['platform']['host_helper_policy'] = None
        original = pathlib.Path.read_text
        def read(path, *a, **k):
            return json.dumps(lock) if path.name == 'inputs.lock.json' else original(path, *a, **k)
        with patch.object(pathlib.Path, 'read_text', read), self.assertRaisesRegex(ValueError, 'host helper policy'):
            validate.validate(platform=True)

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
