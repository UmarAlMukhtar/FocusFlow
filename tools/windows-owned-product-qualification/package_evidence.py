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
        for mode in PIPE_NAMES:
            p=evidence/'core-pipes'/(mode+'.json')
            if p.is_file():candidates.append((p,Path('cases/core-pipes')/p.name))
        for p in (evidence/'fault-tools').glob('profile-relay-*.json'):
            if p.stem.removeprefix('profile-relay-').isdigit():candidates.append((p,Path('cases/relay')/p.name))
    # Diagnostics first; a cap failure remains explicit and never deletes original local evidence.
    for p,relative in sorted(candidates,key=lambda pair:pair[0].name=='screen.mp4'):
        limit=64*1024*1024 if p.name=='screen.mp4' else 4*1024*1024 if p.name=='trace.json' else 4096 if p.name=='source-actions.json' else 16*1024*1024
        n=p.stat().st_size
        if p.is_symlink() or n>limit or total+n>255*1024*1024:omitted.append(relative.as_posix());continue
        target=out/relative;target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(p,target);total+=n
        with p.open('rb') as f:hashes[relative.as_posix()]=hashlib.file_digest(f,'sha256').hexdigest()
    (out/'upload-manifest.json').write_text(json.dumps(dict(bytes=total,sha256=hashes,omitted=omitted,capFailure=bool(omitted)),indent=2)+'\n',encoding='utf-8')
    if omitted:raise RuntimeError('Upload cap hit; qualification remains failed; bounded diagnostic subset retained')
    return hashes
if __name__=='__main__':package(*map(Path,sys.argv[1:]))
