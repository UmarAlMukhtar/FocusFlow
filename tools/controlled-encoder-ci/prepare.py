"""Create the historical recipe's isolated layout from checked inputs; never build."""
import argparse
import json
import os
import pathlib
import shutil
import stat
import subprocess
import tarfile

from acquire import BASE, checked_files, sha


def safe_tar(archive, destination):
    with tarfile.open(archive) as source:
        for member in source:
            path = pathlib.PurePosixPath(member.name)
            if path.is_absolute() or '..' in path.parts:
                raise ValueError('Unsafe archive member: ' + member.name)
            if not (member.isfile() or member.isdir() or member.issym() or member.islnk()):
                raise ValueError('Special archive member: ' + member.name)
        source.extractall(destination, filter='data')


def rpm_entries(package):
    text = subprocess.check_output(['rpm', '-qp', '--dump', str(package)], text=True, timeout=30)
    entries = []
    for line in text.splitlines():
        fields = line.split()
        name, digest, mode, link = fields[0], fields[3], int(fields[4], 8), fields[-1]
        path = pathlib.PurePosixPath(name.lstrip('/'))
        if '..' in path.parts or not path.parts:
            raise ValueError('Unsafe RPM path: ' + name)
        if not (stat.S_ISDIR(mode) or stat.S_ISREG(mode) or stat.S_ISLNK(mode)):
            raise ValueError('Special RPM path: ' + name)
        entries.append((path, digest, mode, link))
    links = {path for path, _, mode, _ in entries if stat.S_ISLNK(mode)}
    if any(parent in links for path, _, _, _ in entries for parent in path.parents):
        raise ValueError('RPM contains a symlink parent')
    return entries


def unpack_rpm(package, destination, rpmdb, fingerprint):
    signature = subprocess.check_output(['rpmkeys', '--dbpath', str(rpmdb), '-Kv', str(package)],
                                        text=True, stderr=subprocess.STDOUT, timeout=30)
    if fingerprint.lower() + ': ok' not in signature.lower():
        raise ValueError('Expected RPM signer missing: ' + package.name)
    entries = rpm_entries(package)
    for rel, _, _, _ in entries:
        if any((destination / parent).is_symlink() for parent in rel.parents):
            raise ValueError('Existing RPM destination has symlink parent')
    # Signed header paths are checked before cpio; no scriptlet or package installation runs.
    source = subprocess.Popen(['rpm2cpio', str(package)], stdout=subprocess.PIPE)
    try:
        subprocess.run(['cpio', '-idm', '--quiet', '--no-absolute-filenames'], cwd=destination,
                       stdin=source.stdout, check=True, timeout=120)
    finally:
        source.stdout.close()
        source.wait(timeout=30)
    if source.returncode:
        raise ValueError('rpm2cpio failed: ' + package.name)
    verified = []
    for rel, digest, mode, link in entries:
        path = destination / rel
        if stat.S_ISLNK(mode):
            if not path.is_symlink() or str(path.readlink()) != link:
                raise ValueError('RPM link mismatch: ' + str(rel))
            target = destination / link.lstrip('/') if link.startswith('/') else path.parent / link
            if not target.resolve().is_relative_to(destination.resolve()):
                raise ValueError('RPM link escapes owned root: ' + str(rel))
            if link.startswith('/'):
                path.unlink()
                path.symlink_to(os.path.relpath(target, path.parent))
        elif stat.S_ISREG(mode):
            if path.is_symlink() or sha(path) != digest:
                raise ValueError('RPM payload mismatch: ' + str(rel))
            verified.append({'path':str(rel), 'sha256':digest, 'rpm':package.name})
    return signature, verified


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache', type=pathlib.Path, required=True)
    parser.add_argument('--work', type=pathlib.Path, required=True)
    parser.add_argument('--plan-only', action='store_true')
    args = parser.parse_args()
    args.cache = args.cache.resolve()
    args.work = args.work.resolve()
    lock = json.loads((BASE / 'inputs.lock.json').read_text())
    if args.work.exists():
        raise ValueError('Refusing existing work directory')
    if sha(BASE / 'build.sh') != lock['adapted_recipe_sha256']:
        raise ValueError('Adapted build recipe changed')
    if args.plan_only:
        print(json.dumps({'work':str(args.work), 'inputs':len(lock['files']), 'build':False,
                          'layout':'target/release-encoder-experiment/candidate', 'jobs':2}))
        return
    checked_files(args.cache, lock)
    args.work.mkdir(parents=True)
    e = args.work / 'target/release-encoder-experiment'
    candidate = e / 'candidate'
    receipts = candidate / 'receipts'
    receipts.mkdir(parents=True)
    sources = e / 'source-inputs'
    (sources / 'sources').mkdir(parents=True)
    mingw = args.work / 'target/windows-verification/gnu-route/mingw'
    nasm = e / 'nasm-tool'
    (nasm / 'prefix').mkdir(parents=True)
    mingw.mkdir(parents=True)
    rpmdb = args.work / 'rpmdb'
    rpmdb.mkdir()
    key = BASE / 'keys/RPM-GPG-KEY-fedora-44-primary'
    for name, record in lock['keys'].items():
        if sha(BASE / 'keys' / name) != record['sha256']:
            raise ValueError('Signing key hash mismatch')
    subprocess.run(['rpmkeys', '--dbpath', str(rpmdb), '--import', str(key)], check=True, timeout=30)
    payloads = []
    for row in lock['files']:
        archive = args.cache / row['name']
        if row['kind'] == 'encoder':
            shutil.copy2(archive, sources / archive.name)
            safe_tar(archive, sources / 'sources')
        elif row['kind'].endswith('rpm'):
            if row['kind'] == 'source-rpm':
                signature = subprocess.check_output(['rpmkeys', '--dbpath', str(rpmdb), '-Kv', str(archive)], text=True, timeout=30)
                if lock['rpm_signer'].lower() not in signature.lower():
                    raise ValueError('Source RPM signer mismatch')
            else:
                signature, verified = unpack_rpm(archive, mingw if row['kind'] == 'mingw-rpm' else nasm / 'prefix', rpmdb, lock['rpm_signer'])
                if row['kind'] == 'mingw-rpm':
                    payloads.extend(verified)
            (receipts / (archive.name + '.signature.txt')).write_text(signature)
    # Verify detached signatures using only checked-in keys and an isolated keyring.
    gpg = args.work / 'gnupg'
    gpg.mkdir(mode=0o700)
    command = ['gpg', '--homedir', str(gpg), '--batch', '--no-auto-key-retrieve']
    subprocess.run([*command, '--import', str(BASE / 'keys/ffmpeg-devel.asc'), str(BASE / 'keys/madler.asc')], check=True, timeout=30)
    for name, key in [('ffmpeg-8.0.3.tar.xz', 'ffmpeg-devel.asc'), ('zlib-1.3.2.tar.xz', 'madler.asc')]:
        result = subprocess.check_output([*command, '--status-fd', '1', '--verify', str(args.cache / (name + '.asc')), str(args.cache / name)], text=True, timeout=30)
        if 'VALIDSIG ' + lock['keys'][key]['fingerprint'] not in result:
            raise ValueError('Archive signer mismatch')
        (receipts / (name + '.signature.txt')).write_text(result)
    for name in ['encoder-source-pins.json', 'source-inventory.json', 'reused-gnu-rpm-verification.json', 'tool-availability.json']:
        data = json.loads((BASE / 'provenance' / name).read_text())
        if name == 'encoder-source-pins.json':
            for row in data['source_pins']:
                row['archive'] = str(sources / pathlib.Path(row['archive']).name)
        if name == 'reused-gnu-rpm-verification.json':
            for row in data['packages']:
                row['path'] = str(args.cache / pathlib.Path(row['path']).name)
        (sources / name).write_text(json.dumps(data, indent=2) + '\n')
    shutil.copy2(BASE / 'provenance/nasm-inputs.json', nasm / 'nasm-inputs.json')
    for name in ['build.sh', 'inspect-candidate.py', 'source_coverage.py']:
        shutil.copy2(BASE / name, candidate / name)
    shutil.copy2(BASE / 'provenance/linux-config_components.h', receipts / 'linux-config_components.h')
    for name in ['coverage-397.json', 'source-index.json', 'include-closure.json']:
        shutil.copy2(BASE / 'provenance' / name, receipts / name)
    shutil.copytree(BASE / 'provenance/audio-profile-extra', receipts / 'audio-profile-extra')
    (receipts / 'rpm-payload-verification.json').write_text(json.dumps(payloads, indent=2) + '\n')
    shutil.copy2(BASE / 'inputs.lock.json', receipts / 'inputs.lock.json')
    print(candidate)


if __name__ == '__main__':
    main()
