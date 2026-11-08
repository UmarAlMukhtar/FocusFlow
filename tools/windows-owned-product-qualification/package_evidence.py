"""Copy only named generated qualification evidence into a bounded upload directory."""
import hashlib
import json
from pathlib import Path
import shutil
import sys

ROOT_NAMES={'source-manifest.json','provenance.json','executables.json','failures.json','artifact-hashes.json','result.json','encoder-hashes.json','runner.json','toolchain.txt','dependency-resolution.json','native-platform.json'}
CASE_NAMES={'source-actions.json','source-request.json','source-ready.json','source-observed.json','source-selection.json','source-exit.json','source-cleanup.json','source.log','source-command-1.json','source-command-2.json','source-ack-1.json','source-ack-2.json','first-write-started.json','media-preservation.json','capture.json','capture.windows-v2','project.json','screen.mp4','trace.json','cleanup.json','owned.json','result.json','analysis.json','paint-history.json','encoder-fixture.json','consumer-prefix.json','metadata.json','session.json','clicks.json','drags.json','timeline.json'}
MODES={'static','repeat','close','resize','admission-close','admission-resize','marker-failure','lost-worker','transfer-shutdown','stop-shutdown','close-stop','actual-executable','blocked','partial','exit7','parent-death','geometry-320x240','geometry-641x359','geometry-803x451','geometry-960x540','geometry-161x479'}
PIPE_NAMES={'valid','idle','stderr','stdout','closed','cancel-idle','cancel-stderr','exit7','hold-stdout','hold-stderr','rows5000'}
COMMANDS={'driver','analyzer','native-platform','core-publication','core-import','mapping-debug','core-pipes','build-cfr_grid_windows','build-FocusFlow','build-windows_owned_qualification','build-windows_encoder_fixture'}|MODES

CURSOR_MODES={'cursor-borderless-even','cursor-decorated-even','cursor-borderless-odd','cursor-decorated-odd'}
PRIVATE_NAMES={'proof-with-cursor.mp4','proof-without-cursor.mp4','proof-summary.json','geometry.json','trace.json','owned.json','first-write-started.json','challenges.json','visibility.json','close-observation.json'}|{f'raw-{i}.pam' for i in range(2)}|{f'cursor-{i}.pam' for i in range(12)}|{f'native-color-{i}.pam' for i in range(12)}|{f'raster-{i}-{j}.pam' for i in range(12) for j in range(4)}
COMMANDS|=CURSOR_MODES|{'cursor-portable','cursor-renderer','cursor-startup-lifecycle','cursor-startup-contract','cursor-startup-gate','startup-metadata'}

def package(evidence,setup,out):
    out.mkdir();total=0;omitted=[];hashes={}
    candidates=[]
    for root in [evidence,setup]:
        if not root.exists():continue
        for p in root.iterdir():
            if p.is_file() and (p.name in ROOT_NAMES or p.name in {n+s for n in COMMANDS for s in ['.log','-command.json']}):candidates.append((p,Path(p.name)))
    if evidence.exists():
        for mode in MODES:
            for i in range(3):
                session=evidence/mode/f'session-{i}'
                for name in CASE_NAMES:
                    p=session/name
                    if p.is_file():candidates.append((p,Path('cases')/mode/f'session-{i}'/name))
        for mode in CURSOR_MODES:
            session=evidence/mode/'session-0'
            for p in session.rglob('*'):
                if p.name in {'capture.json','capture.windows-v2','project.json','screen.mp4','session.json'}:omitted.append(p.relative_to(evidence).as_posix())
            for name in (CASE_NAMES-{'capture.json','capture.windows-v2','project.json','screen.mp4','session.json'})|{'proof-result.json','cursor-analysis.json','native-topology.json','resource-paths.json'}|{f'source-{kind}-{i}.json' for kind in ['command','ack'] for i in range(3,17)}:
                p=session/name
                if p.is_file():candidates.append((p,Path('cases')/mode/'session-0'/name))
            for proof in ['without','with','alpha-without','alpha-with','alpha-probe','challenge','closed','visibility']:
                for name in PRIVATE_NAMES:
                    p=session/proof/name
                    if p.is_file():candidates.append((p,Path('cases')/mode/'session-0'/proof/name))
        session=evidence/'startup-metadata/session-0'
        for p in session.rglob('*'):
            if p.is_file() and (p.suffix in {'.pam','.mp4'} or p.name in {'capture.json','capture.windows-v2','project.json','session.json','first-write-started.json','metadata.json','trace.json','clicks.json','drags.json','timeline.json'}):omitted.append(p.relative_to(evidence).as_posix())
        source_names={'source-request.json','source-ready.json','source-observed.json','source-selection.json','source-exit.json','source-cleanup.json','source-actions.json','source-command-1.json','source-ack-1.json','paint-history.json','source.log'}
        for name in source_names:
            p=session/name
            if p.is_file():candidates.append((p,Path('cases/startup-metadata/session-0')/name))
        for name in ['startup-metadata.json','startup-phase.json','metadata-owned.json','metadata-cleanup.json','startup-analysis.json']:
            p=session/'startup-probe'/name
            if p.is_file():candidates.append((p,Path('cases/startup-metadata/session-0/startup-probe')/name))
        for mode in PIPE_NAMES:
            p=evidence/'core-pipes'/(mode+'.json')
            if p.is_file():candidates.append((p,Path('cases/core-pipes')/p.name))
        for p in (evidence/'fault-tools').glob('profile-relay-*.json'):
            if p.stem.removeprefix('profile-relay-').isdigit():candidates.append((p,Path('cases/relay')/p.name))
    # Diagnostics first; a cap failure remains explicit and never deletes original local evidence.
    for p,relative in sorted(candidates,key=lambda pair:pair[0].suffix=='.mp4'):
        limit=64*1024*1024 if p.name in {'screen.mp4','proof-with-cursor.mp4','proof-without-cursor.mp4'} else 4*1024*1024 if p.name=='trace.json' else 4096 if p.name=='source-actions.json' else 9*1024*1024 if p.suffix=='.pam' else 16*1024*1024
        if p.name in {'startup-metadata.json','startup-analysis.json'}:limit=64*1024
        if p.name in {'startup-phase.json','metadata-owned.json'}:limit=4096
        if p.name=='metadata-cleanup.json':limit=16*1024
        n=p.stat().st_size
        if p.is_symlink() or n>limit or total+n>255*1024*1024:omitted.append(relative.as_posix());continue
        target=out/relative;target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(p,target);total+=n
        with p.open('rb') as f:hashes[relative.as_posix()]=hashlib.file_digest(f,'sha256').hexdigest()
    (out/'upload-manifest.json').write_text(json.dumps(dict(bytes=total,sha256=hashes,omitted=omitted,capFailure=bool(omitted)),indent=2)+'\n',encoding='utf-8')
    if omitted:raise RuntimeError('Upload cap hit; qualification remains failed; bounded diagnostic subset retained')
    return hashes
if __name__=='__main__':package(*map(Path,sys.argv[1:]))
