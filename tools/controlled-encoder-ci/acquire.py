"""Download only reviewed, exact inputs. Never invoked by offline preparation."""
import argparse
import hashlib
import json
import pathlib
import subprocess
import urllib.request

BASE = pathlib.Path(__file__).resolve().parent


def sha(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def checked_files(cache, lock):
    for row in lock['files']:
        path = cache / row['name']
        if path.stat().st_size != row['bytes'] or sha(path) != row['sha256']:
            raise ValueError('Input identity mismatch: ' + row['name'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache', type=pathlib.Path, required=True)
    args = parser.parse_args()
    lock = json.loads((BASE / 'inputs.lock.json').read_text())
    args.cache.mkdir(parents=True, exist_ok=False)
    total = 0
    receipts = []
    for row in lock['files']:
        path = args.cache / row['name']
        if 'git_url' in row:
            gitdir = args.cache / 'x264.git'
            subprocess.run(['git', 'init', '--bare', str(gitdir)], check=True, timeout=30)
            git = ['git', '-c', 'credential.helper=', '-c', 'fetch.fsckObjects=true',
                   '-c', 'transfer.fsckObjects=true', '--git-dir', str(gitdir)]
            subprocess.run([*git, 'fetch', '--depth=1', row['git_url'], row['commit']], check=True, timeout=180)
            actual = subprocess.check_output([*git, 'rev-parse', 'FETCH_HEAD^{tree}'], text=True, timeout=30).strip()
            if actual != row['tree']:
                raise ValueError('x264 Git tree mismatch')
            subprocess.run([*git, 'fsck', '--full', '--strict'], check=True, timeout=30)
            subprocess.run([*git, 'archive', '--format=tar', '--prefix=x264-' + row['commit'] + '/',
                            '--output=' + str(path), row['commit']], check=True, timeout=30)
        else:
            # Counts are bounded even if Content-Length is absent or dishonest.
            count = 0
            with urllib.request.urlopen(row['url'], timeout=45) as response, path.open('xb') as out:
                if not response.url.startswith('https://'):
                    raise ValueError('Non-HTTPS redirect')
                while block := response.read(1024 * 1024):
                    count += len(block)
                    total += len(block)
                    if count > row['bytes'] or total > 512 * 1024**2:
                        raise ValueError('Download exceeds reviewed bounds')
                    out.write(block)
            receipts.append({'name':row['name'], 'requested_url':row['url'], 'effective_url':response.url,
                             'bytes':count, 'sha256':sha(path)})
        if path.stat().st_size != row['bytes'] or sha(path) != row['sha256']:
            raise ValueError('Downloaded input identity mismatch: ' + row['name'])
        (args.cache / 'acquisition.json').write_text(json.dumps(receipts, indent=2) + '\n')
    checked_files(args.cache, lock)


if __name__ == '__main__':
    main()
