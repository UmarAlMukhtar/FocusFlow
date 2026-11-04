import hashlib
import json
import pathlib
import shutil
import tempfile
import unittest
import sys
from unittest.mock import patch

import artifact

import source_coverage
import validate


class ProfileSourceCoverage(unittest.TestCase):
    def fixture(self, root):
        shutil.copy2(validate.BASE / 'provenance/coverage-397.json', root / 'coverage-397.json')
        shutil.copytree(validate.BASE / 'provenance/audio-profile-extra', root / 'audio-profile-extra')
        archive = root / 'libmsvcrt.a'
        archive.write_bytes(b'pinned archive fixture')
        member = b'owned cbrt object fixture'
        receipt = root / 'audio-profile-extra/coverage.json'
        coverage = json.loads(receipt.read_text())
        entry = coverage['entries'][0]
        entry.update(archive_sha256=hashlib.sha256(archive.read_bytes()).hexdigest(),
                     archive_bytes=archive.stat().st_size, member_sha256=hashlib.sha256(member).hexdigest(),
                     member_bytes=len(member))
        receipt.write_text(json.dumps(coverage))
        base = json.loads((root / 'coverage-397.json').read_text())
        selected = {(row['archive'], row['member']) for row in base['entries']
                    if row['kind'] != 'explicit startup object'}
        selected.add(('libmsvcrt.a', 'lib64_libmsvcrt_common_a-cbrt.o'))
        return selected, archive, member

    def test_exact_395_selected_members_accept_only_closed_singleton(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            selected, archive, member = self.fixture(root)
            self.assertEqual(source_coverage.verify_profile_coverage(root, selected, archive, member)
                             ['mapped_selected_archive_members'], 395)

    def test_unknown_and_missing_members_still_fail_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            selected, archive, member = self.fixture(root)
            for wrong in [selected | {('libmsvcrt.a', 'unknown.o')},
                          selected - {('libmsvcrt.a', 'lib64_libmsvcrt_common_a-cbrt.o')},
                          selected - {('libgcc.a', '_chkstk_ms.o')}]:
                with self.subTest(selected=len(wrong)), self.assertRaisesRegex(ValueError, 'selection'):
                    source_coverage.verify_profile_coverage(root, wrong, archive, member)

    def test_source_archive_member_and_base_receipt_tampering_rejected(self):
        for target in ['member', 'archive', 'source', 'base', 'extra-entry']:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as tmp:
                root = pathlib.Path(tmp)
                selected, archive, member = self.fixture(root)
                if target == 'member': member += b'changed'
                elif target == 'archive': archive.write_bytes(b'changed')
                elif target == 'source':
                    (root / 'audio-profile-extra/source-units/mingw-w64-v13.0.0/mingw-w64-crt/math/cbrt.c').write_bytes(b'changed')
                elif target == 'base':
                    (root / 'coverage-397.json').write_text('{}')
                else:
                    receipt = root / 'audio-profile-extra/coverage.json'
                    data = json.loads(receipt.read_text())
                    data['entries'].append({**data['entries'][0], 'member': 'unknown.o'})
                    receipt.write_text(json.dumps(data))
                with self.assertRaises(ValueError):
                    source_coverage.verify_profile_coverage(root, selected, archive, member)

    def test_packed_handoff_retains_extra_source_and_notice_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            candidate = root / 'candidate'
            for name in ['prefix/bin', 'receipts/audio-profile-extra', 'notices/audio-profile-extra']:
                (candidate / name).mkdir(parents=True)
            binary = candidate / 'prefix/bin/ffmpeg.exe'
            binary.write_bytes(b'owned candidate fixture')
            (candidate / 'receipts/candidate.json').write_text(json.dumps({'sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}))
            (candidate / 'receipts/audio-profile-extra/retained-source.c').write_text('source')
            (candidate / 'notices/audio-profile-extra/DISCLAIMER.PD').write_text('notice')
            (candidate / 'build.log').write_text('fixture')
            output = root / 'handoff'
            with patch.object(sys, 'argv', ['artifact.py', 'pack', '--candidate', str(candidate), '--artifact', str(output)]):
                artifact.main()
            self.assertTrue((output / 'candidate/receipts/audio-profile-extra/retained-source.c').is_file())
            self.assertTrue((output / 'candidate/notices/audio-profile-extra/DISCLAIMER.PD').is_file())
            artifact.verify(output)


if __name__ == '__main__':
    unittest.main()
