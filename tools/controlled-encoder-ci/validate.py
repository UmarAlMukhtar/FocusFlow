"""Offline recipe controls and fail-closed platform publication gate."""
import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import shlex

BASE = pathlib.Path(__file__).resolve().parent


def validate_source_receipts(lock, receipts):
    rows = {row['name']:row for row in lock['files']}
    for package in receipts['packages']:
        signed = package['final']
        name = signed['source_rpm']
        row = rows[name]
        for field in ['bytes', 'sha256', 'url']:
            if row[field] != signed[field]:
                raise ValueError(f"Signed source receipt mismatch: {name} {field}: lock={row[field]} signed={signed[field]}")


def validate_audio_profile(recipe):
    # Configure uses pcm_f32le; the CLI raw format is named f32le.
    command = next(line for line in recipe.splitlines() if '/configure" --disable-autodetect' in line)
    options = dict(token[2:].split('=', 1) for token in shlex.split(command)
                   if token.startswith('--enable-') and '=' in token)
    required = {'filter': {'amix', 'asetrate', 'afade'}, 'encoder': {'pcm_f32le'},
                'demuxer': {'pcm_f32le'}, 'muxer': {'pcm_f32le'}}
    for kind, features in required.items():
        enabled = set(options.get('enable-' + kind, '').split(','))
        if not features <= enabled or 'f32le' in enabled:
            raise ValueError('Incomplete audio profile: ' + kind)
    return required


def validate(platform=False):
    lock = json.loads((BASE / 'inputs.lock.json').read_text())
    validate_audio_profile((BASE / 'build.sh').read_text())
    validate_source_receipts(lock, json.loads((BASE / 'provenance/source-rpm-acquisition.json').read_text()))
    names = []
    for row in lock['files']:
        if pathlib.PurePosixPath(row['name']).name != row['name'] or not re.fullmatch('[0-9a-f]{64}', row['sha256']):
            raise ValueError('Unsafe input identity')
        if row['bytes'] <= 0 or row['bytes'] > 256 * 1024**2:
            raise ValueError('Unbounded input')
        if not row.get('url', row.get('git_url', '')).startswith('https://'):
            raise ValueError('HTTPS source route required')
        names.append(row['name'])
    if len(names) != len(set(names)):
        raise ValueError('Duplicate input basename')
    for name, record in lock['keys'].items():
        if hashlib.sha256((BASE / 'keys' / name).read_bytes()).hexdigest() != record['sha256']:
            raise ValueError('Changed public signing key')
    if hashlib.sha256((BASE / 'build.sh').read_bytes()).hexdigest() != lock['adapted_recipe_sha256']:
        raise ValueError('Changed adapted recipe')
    if platform:
        gaps = []
        if not re.fullmatch(r'[^\s]+@sha256:[0-9a-f]{64}', lock['platform'].get('container_image') or ''):
            gaps.append('verified Fedora44 container digest')
        if not lock['platform'].get('host_helper_packages') or not lock['platform'].get('host_helper_policy'):
            gaps.append('reviewed host helper policy')
        if not re.fullmatch('[0-9a-f]{40}', lock['platform'].get('download_artifact_action_sha') or ''):
            gaps.append('verified download-artifact action commit')
        if gaps:
            raise ValueError('Publication blocked: acquire ' + '; '.join(gaps))
    return lock


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', action='store_true')
    parser.add_argument('--host', action='store_true')
    args = parser.parse_args()
    lock = validate(platform=args.platform or args.host)
    if args.host:
        for tool in ['bash', 'python3', 'make', 'gcc', 'pkg-config', 'rpm', 'rpmkeys', 'rpm2cpio', 'cpio', 'gpg', 'git']:
            if not __import__('shutil').which(tool):
                raise ValueError('Required host tool missing: ' + tool)
    print(json.dumps({'status':'PASS', 'platform_checked':args.platform or args.host,
                      'container_image':lock['platform']['container_image'], 'inputs':len(lock['files'])}))


if __name__ == '__main__':
    main()
