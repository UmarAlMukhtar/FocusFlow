"""Exact historical contributions plus the reviewed audio-profile singleton."""
import hashlib
import json
import pathlib


def verify_profile_coverage(receipts, actual_members, archive, member):
    extra = receipts / 'audio-profile-extra'
    coverage = json.loads((extra / 'coverage.json').read_text())
    digest = lambda data: hashlib.sha256(data).hexdigest()
    base_data = (receipts / 'coverage-397.json').read_bytes()
    if digest(base_data) != coverage['base_coverage_sha256']:
        raise ValueError('Historical coverage receipt changed')
    base = json.loads(base_data)['entries']
    expected = {(row['archive'], row['member']) for row in base
                if row['kind'] != 'explicit startup object'}
    entries = coverage['entries']
    if len(entries) != 1 or (entries[0]['archive'], entries[0]['member']) != (
            'libmsvcrt.a', 'lib64_libmsvcrt_common_a-cbrt.o'):
        raise ValueError('Only the reviewed cbrt singleton is authorized')
    entry = entries[0]
    expected.add((entry['archive'], entry['member']))
    if actual_members != expected:
        raise ValueError('External member selection differs: ' + str({
            'new': sorted(actual_members - expected), 'missing': sorted(expected - actual_members)}))
    rows = coverage['files']
    paths = [row['path'] for row in rows]
    actual_paths = {path.relative_to(extra).as_posix() for path in extra.rglob('*')
                    if path.is_file() and path != extra / 'coverage.json'}
    if len(paths) != len(set(paths)) or set(paths) != actual_paths:
        raise ValueError('Extra source/notice file set changed')
    for row in rows:
        relative = pathlib.PurePosixPath(row['path'])
        if relative.is_absolute() or '..' in relative.parts:
            raise ValueError('Unsafe extra source path')
        path = extra / relative
        if path.is_symlink() or path.stat().st_size != row['bytes'] or digest(path.read_bytes()) != row['sha256']:
            raise ValueError('Extra source/notice hash mismatch: ' + row['path'])
    if digest((extra / entry['source']).read_bytes()) != entry['source_sha256']:
        raise ValueError('Extra source unit mismatch')
    if archive.stat().st_size != entry['archive_bytes'] or digest(archive.read_bytes()) != entry['archive_sha256']:
        raise ValueError('Pinned target archive mismatch')
    if len(member) != entry['member_bytes'] or digest(member) != entry['member_sha256']:
        raise ValueError('Pinned target member mismatch')
    return {'status': 'PASS', 'base_mapped_archive_members': len(expected) - 1,
            'profile_additions': 1, 'mapped_selected_archive_members': len(actual_members),
            'source_map': 'coverage-397.json + audio-profile-extra/coverage.json',
            'pending_header_closure_preserved': True, 'legal_completeness_claimed': False}
