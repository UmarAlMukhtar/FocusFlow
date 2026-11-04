"""Bounded generated audio capability checks; no capture or mapping algorithm."""
import argparse
import hashlib
import json
import math
import pathlib
import struct
import subprocess
import sys


def pcm_samples(data, frames, channels):
    if not 1 <= frames <= 96000 or channels not in (1, 2):
        raise ValueError('PCM fixture bounds exceeded')
    if len(data) != frames * channels * 4:
        raise ValueError('PCM frame/channel byte count mismatch')
    samples = struct.unpack('<' + str(frames * channels) + 'f', data)
    if not all(math.isfinite(value) for value in samples):
        raise ValueError('Nonfinite PCM output')
    return samples


def check_pcm(actual, expected):
    if len(actual) != len(expected) or any(not math.isfinite(a) or abs(a - b) > 1e-6
                                          for a, b in zip(actual, expected)):
        raise ValueError('PCM amplitude/channel mismatch')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    binary = args.candidate.resolve()
    if sys.platform != 'win32' or not binary.is_file():
        raise ValueError('Requires an explicit owned Windows candidate on Windows')
    args.output.mkdir(parents=True, exist_ok=False)
    output = args.output.resolve()
    records = []
    passed = False
    base = ['-hide_banner', '-loglevel', 'error', '-nostdin', '-n']

    def run(name, options):
        argv = [str(binary), *map(str, options)]
        record = {'name': name, 'argv': argv, 'exit': None, 'timed_out': False}
        records.append(record)
        try:
            completed = subprocess.run(argv, capture_output=True, timeout=20)
        except subprocess.TimeoutExpired as error:
            record['timed_out'] = True
            (output / (name + '.stdout')).write_bytes(error.stdout or b'')
            (output / (name + '.stderr')).write_bytes(error.stderr or b'')
            raise
        record['exit'] = completed.returncode
        (output / (name + '.stdout')).write_bytes(completed.stdout)
        (output / (name + '.stderr')).write_bytes(completed.stderr)
        if completed.returncode:
            raise ValueError('Candidate audio command failed: ' + name)
        return completed.stdout.decode(errors='replace')

    def inp(path):
        return ['-protocol_whitelist', 'file,pipe', '-f', 'f32le', '-ar', '48000',
                '-ac', '2', '-i', path]

    def render(name, inputs, graph, frames):
        path = output / (name + '.f32le')
        run(name, [*base, *inputs, '-filter_complex', graph, '-map', '[a]',
                   '-c:a', 'pcm_f32le', '-f', 'f32le', path])
        if path.stat().st_size != frames * 2 * 4:
            raise ValueError('Unexpected PCM size: ' + name)
        return pcm_samples(path.read_bytes(), frames, 2)

    try:
        run('version', ['-version'])
        for flag, required in [('filters', ['amix', 'asetrate', 'afade']),
                               ('encoders', ['pcm_f32le']), ('demuxers', ['f32le']),
                               ('muxers', ['f32le'])]:
            listing = run('startup-' + flag, ['-hide_banner', '-' + flag])
            for feature in required:
                if not any(feature in line.split() for line in listing.splitlines()):
                    raise ValueError('Missing candidate capability: ' + feature)
        first = output / 'first.f32le'
        second = output / 'short-sibling.f32le'
        first.write_bytes(struct.pack('<2f', .25, -.5) * 1024)
        second.write_bytes(struct.pack('<2f', -.125, .25) * 512)
        # Reopen the first command's private output in the following mix command.
        check_pcm(render('roundtrip', inp(first), '[0:a]aformat=sample_fmts=flt[a]', 1024),
             (.25, -.5) * 1024)
        mixed = render('fixed-mix', [*inp(output / 'roundtrip.f32le'), *inp(second)],
                       "[0:a][1:a]amix=inputs=2:weights='0.5 0.5':normalize=0:dropout_transition=0:duration=longest[a]", 1024)
        check_pcm(mixed, (.0625, -.125) * 512 + (.125, -.25) * 512)
        faded = render('complementary-fades', [*inp(first), *inp(first)],
                       "[0:a]afade=t=out:ss=0:ns=256:curve=tri[o];"
                       "[1:a]afade=t=in:ss=0:ns=256:curve=tri[n];"
                       "[o][n]amix=inputs=2:weights='1 1':normalize=0:dropout_transition=0:duration=longest[a]", 1024)
        check_pcm(faded, (.25, -.5) * 1024)
        # Simple 2x rate smoke only; this does not implement the fixed-Q mapper.
        rated = render('rate-smoke', inp(first), '[0:a]asetrate=24000,aresample=48000[a]', 2048)
        check_pcm(rated[256:-256], (.25, -.5) * (2048 - 256))
        passed = True
    finally:
        result = {'passed': passed, 'candidate_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                  'commands': records, 'fixture_max_frames': 2048, 'channels': 2,
                  'pcm_format': 'interleaved little-endian float32', 'sample_rate': 48000,
                  'mapper_algorithm_qualified': False, 'native_capture_qualified': False}
        (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        manifest = [{'path': p.relative_to(output).as_posix(), 'bytes': p.stat().st_size,
                     'sha256': hashlib.sha256(p.read_bytes()).hexdigest()}
                    for p in sorted(output.iterdir()) if p.is_file()]
        (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print('Bounded raw-float audio profile checks passed')


if __name__ == '__main__':
    main()
