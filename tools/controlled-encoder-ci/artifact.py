"""Explicit build handoff and cross-job checksum verification."""
import argparse
import json
import pathlib
import shutil

from acquire import BASE, sha


def verify(root):
    if any(path.is_symlink() for path in root.rglob('*')):
        raise ValueError('Artifact contains a symlink')
    manifest = json.loads((root / 'manifest.json').read_text())
    actual = sorted(str(p.relative_to(root)).replace('\\', '/') for p in root.rglob('*') if p.is_file() and p != root / 'manifest.json')
    expected = sorted(item['path'] for item in manifest)
    if actual != expected:
        raise ValueError('Artifact file set differs from manifest')
    for row in manifest:
        rel = pathlib.PurePosixPath(row['path'])
        if rel.is_absolute() or '..' in rel.parts:
            raise ValueError('Unsafe artifact path')
        path = root / rel
        if path.is_symlink() or path.stat().st_size != row['bytes'] or sha(path) != row['sha256']:
            raise ValueError('Artifact hash mismatch: ' + row['path'])
    pe = root / 'candidate/ffmpeg.exe'
    identity = json.loads((root / 'candidate/receipts/candidate.json').read_text())
    if sha(pe) != identity['sha256']:
        raise ValueError('Candidate receipt/executable mismatch')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['pack', 'verify'])
    parser.add_argument('--artifact', type=pathlib.Path, required=True)
    parser.add_argument('--candidate', type=pathlib.Path)
    args = parser.parse_args()
    if args.mode == 'pack':
        args.artifact.mkdir(parents=True, exist_ok=False)
        out = args.artifact / 'candidate'
        (out / 'receipts').mkdir(parents=True)
        shutil.copy2(args.candidate / 'prefix/bin/ffmpeg.exe', out / 'ffmpeg.exe')
        for path in (args.candidate / 'receipts').iterdir():
            if path.is_file() and path.suffix in ['.json', '.txt', '.mak', '.h', '.log', '.map', '.sha256']:
                shutil.copy2(path, out / 'receipts' / path.name)
        shutil.copytree(args.candidate / 'receipts/audio-profile-extra', out / 'receipts/audio-profile-extra')
        shutil.copytree(args.candidate / 'notices', out / 'notices')
        shutil.copy2(args.candidate / 'build.log', out / 'receipts/build.log')
        shutil.copytree(BASE, args.artifact / 'recipe', ignore=shutil.ignore_patterns('__pycache__', '*.pyc'))
        manifest = [{'path':str(path.relative_to(args.artifact)), 'bytes':path.stat().st_size, 'sha256':sha(path)}
                    for path in sorted(args.artifact.rglob('*')) if path.is_file()]
        (args.artifact / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    verify(args.artifact)
    print('Artifact identity verified')


if __name__ == '__main__':
    main()
