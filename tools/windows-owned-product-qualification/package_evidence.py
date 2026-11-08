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
PRIVATE_NAMES={'proof-with-cursor.mp4','proof-without-cursor.mp4','proof-summary.json','geometry.json','trace.json','owned.json','first-write-started.json','challenges.json','visibility.json','close-observation.json','resize-observation.json'}|{f'raw-{i}.pam' for i in range(2)}|{f'cursor-{i}.pam' for i in range(12)}|{f'native-color-{i}.pam' for i in range(12)}|{f'raster-{i}-{j}.pam' for i in range(12) for j in range(4)}
COMMANDS|=CURSOR_MODES|{'cursor-portable','cursor-renderer','cursor-startup-lifecycle','cursor-startup-contract','cursor-startup-gate','startup-metadata'}

PRODUCTION_MODES={f'production-{family}-{grid}-r{repeat}' for family in ['popup','decorated'] for grid in ['even','odd'] for repeat in range(2)}
PRODUCTION_SOURCE={name:4096 for name in ['source-request.json','source-ready.json','source-observed.json','source-selection.json','source-exit.json','source-cleanup.json','source-actions.json','source-command-1.json','source-ack-1.json','paint-history.json']}
PRODUCTION_SOURCE['source.log']=16*1024*1024
PRODUCTION_ROOT={name:4096 for name in ['production-source-ready.json','production-start-authorized.json','production-actions-complete.json','production-placement.json','production-geometry.json','production-armed.json','production-observer-completion.json','production-source-receipt.json','production-phase.json']}
PRODUCTION_ROOT.update({'production-sampled-interactions.json':8*1024*1024,'production-analysis.json':4*1024*1024,'production-media-check.json':4*1024*1024})
PRODUCTION_SESSION={name:4096 for name in ['capture.windows-v3','cleanup.json','owned.json','first-write-started.json','media-preservation.json','handles-open','before-publish','media-hashed']}
PRODUCTION_SESSION.update({name:4*1024*1024 for name in ['capture.json','project.json','trace.json','metadata.json','session.json']})
PRODUCTION_SESSION.update({name:8*1024*1024 for name in ['clicks.json','drags.json','timeline.json']})
PRODUCTION_SESSION.update({'screen.mp4':64*1024*1024,'raw-0.pam':9*1024*1024,'raw-1.pam':9*1024*1024,'cursor/index.json':65536,'cursor/resources.json':65536,'cursor/observations.ndjson':128*1024*1024,'cursor/input-observations.ndjson':128*1024*1024,'cursor/shapes/shape-0.pam':9*1024*1024})
PRODUCTION_FILES={**PRODUCTION_ROOT,**{'source/'+name:cap for name,cap in PRODUCTION_SOURCE.items()},**{'session-0/'+name:cap for name,cap in PRODUCTION_SESSION.items()},**{'native-raster-oracle/'+name:9*1024*1024 for name in ['cursor-0.pam']+[f'raster-0-{i}.pam' for i in range(4)]},'native-raster-oracle/shape.json':4096,'.locks/session-0.document':0}
PRODUCTION_DIRECTORIES={'.locks','source','session-0','session-0/cursor','session-0/cursor/shapes','native-raster-oracle'}
PRODUCTION_INTERNAL={'.locks/session-0.document','session-0/handles-open','session-0/before-publish','session-0/media-hashed'}
PRODUCTION_REQUIRED=set(PRODUCTION_FILES)-PRODUCTION_INTERNAL-{'session-0/metadata.json','session-0/session.json'}
COMMANDS|=PRODUCTION_MODES


def production_transient(relative,parent_pid=None):
    import re
    name=relative.as_posix()
    fixture={p.removesuffix('.json')+'.pending':cap for p,cap in PRODUCTION_FILES.items() if p.endswith('.json') and (p.startswith('source/') and p!='source/paint-history.json' or '/' not in p and p in PRODUCTION_ROOT and p not in {'production-sampled-interactions.json','production-analysis.json','production-media-check.json'})}
    if name in fixture:return fixture[name]
    if name in {'session-0/'+kind+'.json.tmp' for kind in ['clicks','drags','timeline']} and parent_pid is not None:return 8*1024*1024
    match=re.fullmatch(r'session-0/\.(capture|project)\.json-([1-9][0-9]*)-(0|[1-9][0-9]*)\.tmp',name)
    if match and parent_pid is not None and int(match[2])==parent_pid and len(match[3])<=20 and int(match[3])<2**64:return 4*1024*1024
    return None


def production_limit(relative,parent_pid=None):
    name=relative.as_posix()
    if name in PRODUCTION_FILES:return PRODUCTION_FILES[name]
    if name in PRODUCTION_DIRECTORIES:return None
    cap=production_transient(relative,parent_pid)
    assert cap is not None and parent_pid is not None,'unknown production evidence path: '+name
    return cap


def production_files(root,complete=False):
    import production_cursor_check as check
    files=[];records=stream_bytes=0
    for path in root.rglob('*'):
        relative=path.relative_to(root);cap=production_limit(relative)
        import run
        run.production_file(path,root)
        if cap is None:assert path.is_dir();continue
        check.safe_file(root,path,cap)
        if path.name in {'observations.ndjson','input-observations.ndjson'}:
            limit=2048 if path.name=='observations.ndjson' else 1024
            with path.open('rb') as stream:
                for line in iter(lambda:stream.readline(limit+1),b''):
                    records+=1;stream_bytes+=len(line)
                    assert records<=8192 and len(line)<=limit and stream_bytes<=128*1024*1024,'production stream cap'
        if relative.as_posix() not in PRODUCTION_INTERNAL:files.append(path)
    index=root/'session-0/cursor/index.json'
    if index.exists():
        referenced=check.shape_paths(root,check.load(index))
        assert set((root/'session-0/cursor/shapes').iterdir())==set(referenced),'unreferenced production shape'
    else:assert not (root/'session-0/cursor/shapes/shape-0.pam').exists(),'shape without index'
    if complete:assert PRODUCTION_REQUIRED<={p.relative_to(root).as_posix() for p in files},'missing completed production evidence'
    return files


def package(evidence,setup,out):
    out.mkdir();total=0;omitted=[];hashes={};excluded_internal=[]
    candidates=[]
    for root in [evidence,setup]:
        if not root.exists():continue
        for p in root.iterdir():
            if p.is_file() and (p.name in ROOT_NAMES or p.name in {n+s for n in COMMANDS for s in ['.log','-command.json']}):candidates.append((p,Path(p.name)))
    if evidence.exists():
        import production_cursor_check as check
        result_path=evidence/'result.json'
        static_complete=result_path.exists() and check.load(result_path).get('productionStaticCasesPassed') is True
        if static_complete:
            for mode in PRODUCTION_MODES:
                if not (evidence/mode).is_dir():omitted.append(mode+': missing completed production case')
        for mode in PRODUCTION_MODES:
            case=evidence/mode
            if not case.exists():continue
            try:
                import production_cursor_check as check
                complete=static_complete or (case/'production-analysis.json').exists() and check.load(case/'production-analysis.json',4*1024*1024).get('productionStaticCasesPassed') is True
                candidates.extend((p,Path('cases')/mode/p.relative_to(case)) for p in production_files(case,complete))
                excluded_internal.extend(mode+'/'+name for name in sorted(PRODUCTION_INTERNAL) if (case/name).exists())
            except (AssertionError,OSError,ValueError,KeyError) as error:
                omitted.append(mode+': '+str(error))
        for mode in MODES:
            for i in range(3):
                session=evidence/mode/f'session-{i}'
                for name in CASE_NAMES:
                    p=session/name
                    if p.is_file():candidates.append((p,Path('cases')/mode/f'session-{i}'/name))
        for mode in CURSOR_MODES:
            session=evidence/mode/'session-0'
            for p in session.rglob('*'):
                if p.name in {'capture.json','capture.windows-v2','capture.windows-v3','project.json','screen.mp4','session.json'}:omitted.append(p.relative_to(evidence).as_posix())
            for name in (CASE_NAMES-{'capture.json','capture.windows-v2','project.json','screen.mp4','session.json'})|{'proof-result.json','cursor-analysis.json','native-topology.json','resource-paths.json'}|{f'source-{kind}-{i}.json' for kind in ['command','ack'] for i in range(3,17)}:
                p=session/name
                if p.is_file():candidates.append((p,Path('cases')/mode/'session-0'/name))
            for proof in ['without','with','alpha-without','alpha-with','alpha-probe','challenge','resize','closed','visibility']:
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
        parts=relative.parts
        if len(parts)>2 and parts[0]=='cases' and parts[1] in PRODUCTION_MODES:limit=PRODUCTION_FILES['/'.join(parts[2:])]
        n=p.stat().st_size
        if p.is_symlink() or n>limit or total+n>255*1024*1024:omitted.append(relative.as_posix());continue
        target=out/relative;target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(p,target);total+=n
        with p.open('rb') as f:hashes[relative.as_posix()]=hashlib.file_digest(f,'sha256').hexdigest()
    (out/'upload-manifest.json').write_text(json.dumps(dict(bytes=total,sha256=hashes,omitted=omitted,excludedInternal=excluded_internal,capFailure=bool(omitted)),indent=2)+'\n',encoding='utf-8')
    if omitted:raise RuntimeError('Upload cap hit; qualification remains failed; bounded diagnostic subset retained')
    return hashes
if __name__=='__main__':package(*map(Path,sys.argv[1:]))
