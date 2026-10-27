"""Run only generated-window scenarios; a failed or unavailable native lane exits nonzero."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
from analyze import analyze


def run(exe, evidence, ffprobe, ffmpeg):
    if os.name != 'nt':
        raise RuntimeError('native Windows evidence requires execution on Windows')
    evidence = Path(evidence)
    evidence.mkdir(parents=True, exist_ok=False)
    summary = dict(nativeWindowsExecution=True, authorityAccepted=False,
                   platform=platform.platform(), python=platform.python_version(), scenarios=[])
    for scenario in ('first', 'static', 'animated', 'stall', 'close'):
        case = evidence/scenario
        item = dict(scenario=scenario, status='native-unavailable-or-failed')
        with (evidence/(scenario+'-parent.jsonl')).open('wb') as stdout, \
                (evidence/(scenario+'-parent.stderr')).open('wb') as stderr:
            process = subprocess.Popen([exe, 'run', scenario, str(case)], stdout=stdout, stderr=stderr)
            try:
                item['nativeExitCode'] = process.wait(timeout=28)
            except subprocess.TimeoutExpired:
                # Killing this parent closes its non-inherited KILL_ON_JOB_CLOSE handle.
                process.kill()
                item['deadlineExceeded'] = True
                try:
                    item['nativeExitCode'] = process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    item['terminationUnconfirmed'] = True
        if item.get('nativeExitCode') == 0:
            try:
                result = analyze(case, scenario, ffprobe, ffmpeg)
                item['status'] = result['status']
            except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
                item.update(status='analysis-failed', error=str(error))
                (case/'analysis.json').write_text(json.dumps(item, indent=2)+'\n')
        summary['scenarios'].append(item)
        (evidence/'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    # Hash only this run's generated files, never enumerate the workspace or the desktop.
    hashes = {str(p.relative_to(evidence)): hashlib.sha256(p.read_bytes()).hexdigest()
              for p in sorted(evidence.rglob('*')) if p.is_file()}
    (evidence/'evidence-sha256.json').write_text(json.dumps(hashes, indent=2)+'\n')
    print(json.dumps(summary, indent=2))
    return 0 if all(s['status'] == 'observed-preservation' for s in summary['scenarios']) else 2


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', required=True)
    parser.add_argument('--evidence', required=True)
    parser.add_argument('--ffprobe', required=True)
    parser.add_argument('--ffmpeg', required=True)
    args = parser.parse_args()
    raise SystemExit(run(args.exe, args.evidence, args.ffprobe, args.ffmpeg))
