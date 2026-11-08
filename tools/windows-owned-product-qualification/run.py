"""Reviewed generated native lane. Only owned fixtures; no user input, release or remote changes."""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import time
import traceback
import analyze

ARCHIVE_SHA='e2aaeaa0fdbc397d4794828086424d4aaa2102cef1fb6874f6ffd29c0b88b673'
FFMPEG_SHA='5af82a0d4fe2b9eae211b967332ea97edfc51c6b328ca35b827e73eac560dc0d'
FFPROBE_SHA='192a1d6899059765ac8c39764fc3148d4e6049955956dc2029f81f4bd6a8972d'
WINDOWS_IMPORT_TEST_COUNT=15
ALLOW_ROOTS=['FocusFlow/src-tauri/src','FocusFlow/src-tauri/tests','FocusFlow/src-tauri/capabilities','FocusFlow/src-tauri/icons','FocusFlow/crates/focusflow-core/src','FocusFlow/crates/focusflow-core/tests','FocusFlow/vendor/windows-capture','tools/windows-owned-product-qualification']
ALLOW_FILES=['README.md','FocusFlow/src-tauri/Cargo.toml','FocusFlow/src-tauri/Cargo.lock','FocusFlow/src-tauri/build.rs','FocusFlow/src-tauri/tauri.conf.json','FocusFlow/crates/focusflow-core/Cargo.toml','FocusFlow/crates/focusflow-core/Cargo.lock','.superpowers/sdd/2026-10-27-windows-owned-product/fix-1-native-harness/Cargo.toml','.superpowers/sdd/2026-10-27-windows-owned-product/fix-1-native-harness/Cargo.lock','.github/workflows/windows-owned-product-qualification.yml']
PRODUCTION_STATIC_CASES=[(prefix+'-r'+str(repetition),w,h,nonce+4*repetition,profile) for repetition in range(2) for prefix,w,h,nonce,profile in [('production-popup-even',320,240,700,'popup'),('production-decorated-even',320,240,701,'decorated'),('production-popup-odd',641,359,702,'popup'),('production-decorated-odd',641,359,703,'decorated')]]
EXTENSIONS={'.rs','.toml','.lock','.md','.json','.png','.ico','.icns','.py','.yml','.txt','.orig'}

def require_cursor_disposable_context():
    expected=dict(GITHUB_ACTIONS='true',RUNNER_ENVIRONMENT='github-hosted',RUNNER_OS='Windows',
                  GITHUB_REPOSITORY='UmarAlMukhtar/FocusFlow',GITHUB_REF='refs/heads/codex-windows-cursor-qualification',
                  GITHUB_EVENT_NAME='push',GITHUB_WORKFLOW_REF='UmarAlMukhtar/FocusFlow/.github/workflows/windows-owned-product-qualification.yml@refs/heads/codex-windows-cursor-qualification')
    for key,value in expected.items():assert os.getenv(key)==value,f'private cursor disposable context rejected {key}'
    commit=os.getenv('GITHUB_SHA','');assert re.fullmatch('[0-9a-f]{40}',commit),'invalid run SHA'
    assert os.getenv('FOCUSFLOW_CURSOR_REVIEWED_SHA')==commit,'private cursor SHA handshake rejected'
    assert os.getenv('FOCUSFLOW_CURSOR_REVIEWED_REPOSITORY')==expected['GITHUB_REPOSITORY'],'repository handshake rejected'
    assert os.getenv('FOCUSFLOW_CURSOR_REVIEWED_REF')==expected['GITHUB_REF'],'ref handshake rejected'
    run_id=os.getenv('GITHUB_RUN_ID','');attempt=os.getenv('GITHUB_RUN_ATTEMPT','')
    assert run_id.isdigit() and attempt.isdigit(),'run identity missing'
    assert os.getenv('FOCUSFLOW_CURSOR_REVIEWED_RUN')==run_id+':'+attempt,'run handshake rejected'
    return dict(repository=expected['GITHUB_REPOSITORY'],ref=expected['GITHUB_REF'],sha=commit,runId=run_id,attempt=attempt,runnerEnvironment='github-hosted')

def require_metadata_disposable_context():
    expected=dict(GITHUB_ACTIONS='true',RUNNER_ENVIRONMENT='github-hosted',RUNNER_OS='Windows',GITHUB_REPOSITORY='UmarAlMukhtar/FocusFlow',GITHUB_REF='refs/heads/codex-windows-startup-metadata',GITHUB_EVENT_NAME='push',GITHUB_WORKFLOW_REF='UmarAlMukhtar/FocusFlow/.github/workflows/windows-owned-product-qualification.yml@refs/heads/codex-windows-startup-metadata')
    for key,value in expected.items():assert os.getenv(key)==value,f'metadata disposable context rejected {key}'
    commit=os.getenv('GITHUB_SHA','');assert re.fullmatch('[0-9a-f]{40}',commit),'invalid metadata SHA'
    assert os.getenv('FOCUSFLOW_METADATA_REVIEWED_SHA')==commit,'metadata SHA handshake rejected'
    assert os.getenv('FOCUSFLOW_METADATA_REVIEWED_REPOSITORY')==expected['GITHUB_REPOSITORY'],'metadata repository handshake rejected'
    assert os.getenv('FOCUSFLOW_METADATA_REVIEWED_REF')==expected['GITHUB_REF'],'metadata ref handshake rejected'
    run_id=os.getenv('GITHUB_RUN_ID','');attempt=os.getenv('GITHUB_RUN_ATTEMPT','');assert run_id.isdigit() and attempt.isdigit(),'metadata run identity missing'
    assert os.getenv('FOCUSFLOW_METADATA_REVIEWED_RUN')==run_id+':'+attempt,'metadata run handshake rejected'
    return dict(repository=expected['GITHUB_REPOSITORY'],ref=expected['GITHUB_REF'],sha=commit,runId=run_id,attempt=attempt,runnerEnvironment='github-hosted')

def sha(path):
    with path.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()

def source_manifest(repo):
    files=[repo/p for p in ALLOW_FILES]
    for name in ALLOW_ROOTS:
        for p in (repo/name).rglob('*'):
            if p.is_file() and not any(x in {'target','__pycache__','.git','Recordings','evidence'} for x in p.parts) and (p.suffix in EXTENSIONS or p.name in {'LICENCE','LICENSE','LICENSE-MIT','LICENSE-APACHE'}):files.append(p)
    assert all(p.is_file() and not p.is_symlink() for p in files)
    return {p.relative_to(repo).as_posix():sha(p) for p in sorted(set(files))}

def write(path,value):path.write_text(json.dumps(value,indent=2)+'\n',encoding='utf-8')

def require_test_count(text,count,names=()):
    assert re.search(r'test result: ok\. '+str(count)+r' passed; 0 failed; 0 ignored;',text), 'required native tests did not execute without skips'
    for name in names:assert re.search(r'^test '+re.escape(name)+r' \.\.\. ok$',text,re.MULTILINE),'required test did not pass: '+name

def require_exit7(case):
    trace=json.loads((case/'trace.json').read_text(encoding='utf-8'))
    prefix=json.loads((case/'consumer-prefix.json').read_text(encoding='utf-8'))
    assert trace['encoderExitCode']==7 and trace['nonEncoderFailure'] is False and trace['failure']=='Encoder exited 7', 'exit7 was not the sole encoder exit failure'
    assert trace['supervisorTerminated'] is None and trace['overflow'] is False, 'exit7 had Supervisor or observation failure'
    assert trace['selected']==trace['written']>0 and len(trace['writes'])==trace['written'], 'exit7 did not drain every selected frame'
    frame_bytes=(trace['width']+trace['width']%2)*(trace['height']+trace['height']%2)*4
    assert all(w['ok'] is True and w['bytes']==frame_bytes for w in trace['writes']), 'exit7 had an incomplete writer result'
    assert prefix['eof'] is True and prefix['notWriterProgress'] is True and prefix['consumerBytes']==trace['written']*frame_bytes, 'exit7 consumer did not reach EOF after all frames'

def bound(root,production_parent=None):
    import package_evidence
    if production_parent is not None:assert production_parent[0] in package_evidence.PRODUCTION_MODES and type(production_parent[1]) is int and production_parent[1]>0
    size=0;production_records={}
    for p in root.rglob('*'):
        is_link=False
        try:
            is_link=p.is_symlink()
            relative=p.relative_to(root).parts
            if relative and relative[0] in package_evidence.PRODUCTION_MODES:
                production_file(p,root/relative[0])
                if len(relative)>1:
                    parent=production_parent[1] if production_parent is not None and production_parent[0]==relative[0] else None
                    limit=package_evidence.production_limit(Path(*relative[1:]),parent)
                    if limit is None:assert p.is_dir()
                    else:assert p.is_file() and p.stat().st_size<=limit,'production file cap'

            if not p.is_file():continue
            n=p.stat().st_size
        except FileNotFoundError:
            relative=p.relative_to(root).parts
            if len(relative)>1 and relative[0] in package_evidence.PRODUCTION_MODES and production_parent is not None and production_parent[0]==relative[0] and not is_link:
                if package_evidence.production_transient(Path(*relative[1:]),production_parent[1]) is not None:continue
            if len(relative)==3 and relative[0] in {row[0] for row in PRODUCTION_STATIC_CASES} and relative[1]=='source'  and relative[2] in {name+'.pending' for name in ['source-request','source-ready','source-observed','source-selection','source-exit','source-cleanup','source-actions','source-command-1','source-ack-1']} and not is_link:continue
            if len(relative)==2 and relative[0] in {row[0] for row in PRODUCTION_STATIC_CASES} and relative[1] in {name+'.pending' for name in ['production-source-ready','production-start-authorized','production-actions-complete','production-placement','production-geometry','production-armed','production-observer-completion','production-source-receipt','production-phase']} and not is_link:continue
            # The owned initial handshake atomically renames this one transient file.
            relative=p.relative_to(root).parts
            if len(relative)==3 and relative[0] in {'cursor-borderless-even','cursor-decorated-even','cursor-borderless-odd','cursor-decorated-odd'} and relative[1:]==('session-0','source-request.pending') and not is_link:continue
            if not is_link and relative[:2]==('startup-metadata','session-0'):
                if len(relative)==3 and relative[2] in {f'{n}.pending' for n in ['source-request','source-ready','source-observed','source-selection','source-exit','source-cleanup','source-actions','source-command-1','source-ack-1']}:continue
                if len(relative)==4 and relative[2]=='startup-probe' and relative[3] in {'startup-phase.pending','startup-metadata.pending','metadata-owned.pending','metadata-cleanup.pending'}:continue
            raise
        size+=n
        relative=p.relative_to(root).parts
        if relative and relative[0] in {row[0] for row in PRODUCTION_STATIC_CASES}:
            production_file(p,root/relative[0])
            if p.name.startswith('production-') and p.suffix in {'.json','.pending'}:
                assert n<=package_evidence.PRODUCTION_ROOT.get(p.name,4096),'production receipt cap'
            if p.name in {'observations.ndjson','input-observations.ndjson'}:
                assert n<=128*1024*1024,'production stream cap'
                limit=2048 if p.name=='observations.ndjson' else 1024
                with p.open('rb') as stream:
                    for line in iter(lambda:stream.readline(limit+1),b''):
                        production_records[relative[0]]=production_records.get(relative[0],0)+1
                        assert production_records[relative[0]]<=8192 and len(line)<=limit,'production stream record cap'
            if p.name in {'index.json','resources.json'}:assert n<=64*1024,'production index/resource cap'
        if p.name in {'startup-metadata.json','startup-analysis.json'}:assert n<=64*1024,'64KiB metadata cap reached'
        if p.name in {'startup-phase.json','metadata-owned.json'}:assert n<=4096,'4KiB metadata control cap reached'
        if p.name=='metadata-cleanup.json':assert n<=16*1024,'16KiB metadata cleanup cap reached'
        if p.name in {'screen.mp4','proof-with-cursor.mp4','proof-without-cursor.mp4'}:assert n<=64*1024*1024,'64MiB media cap reached'
        elif p.name=='trace.json':assert n<=4*1024*1024,'4MiB trace cap reached'
        elif p.suffix=='.pam':assert n<=9*1024*1024,'9MiB raw PAM cap reached'
        elif p.name=='source-actions.json':assert n<=4096,'4KiB source action cap reached'
        else:assert n<=16*1024*1024,f'16MiB evidence file cap reached: {p.name}'
    assert size<=256*1024*1024,'256MiB aggregate artifact cap reached'

class ExactHandle:
    def __init__(self,pid):
        self.k=ctypes.WinDLL('kernel32',use_last_error=True)
        self.k.OpenProcess.argtypes=[wintypes.DWORD,wintypes.BOOL,wintypes.DWORD];self.k.OpenProcess.restype=wintypes.HANDLE
        self.k.WaitForSingleObject.argtypes=[wintypes.HANDLE,wintypes.DWORD];self.k.WaitForSingleObject.restype=wintypes.DWORD
        self.k.CloseHandle.argtypes=[wintypes.HANDLE];self.k.TerminateProcess.argtypes=[wintypes.HANDLE,wintypes.UINT]
        self.raw=self.k.OpenProcess(0x100000|0x1000|1,False,pid);self.pid=pid
        assert self.raw,ctypes.WinError(ctypes.get_last_error())
        assert not self.signaled(),'child exited before independent observation'
    def signaled(self):return self.k.WaitForSingleObject(self.raw,0)==0
    def kill(self):self.k.TerminateProcess(self.raw,93)
    def close(self):self.k.CloseHandle(self.raw)

def native_qpc():
    value=ctypes.c_longlong()
    assert ctypes.WinDLL('kernel32',use_last_error=True).QueryPerformanceCounter(ctypes.byref(value))
    return value.value

def fixture_write(path,value):
    pending=path.with_suffix('.pending');write(pending,value);pending.replace(path)

def fixture_read(path):
    assert path.is_file() and not path.is_symlink() and path.stat().st_size<=1024,'source control must be a regular file <=1KiB'
    return json.loads(path.read_text(encoding='utf-8'))

def window_pid(hwnd):
    user=ctypes.WinDLL('user32',use_last_error=True)
    user.GetWindowThreadProcessId.argtypes=[wintypes.HWND,ctypes.POINTER(wintypes.DWORD)]
    user.GetWindowThreadProcessId.restype=wintypes.DWORD
    pid=wintypes.DWORD();assert user.GetWindowThreadProcessId(hwnd,ctypes.byref(pid)),'source HWND unavailable'
    return pid.value

def production_tuple(root):
    cases={label:(w,h,nonce,profile) for label,w,h,nonce,profile in PRODUCTION_STATIC_CASES}
    assert root.name in cases,'unexpected production static case'
    return cases[root.name]

def production_file(path,root):
    assert path.is_relative_to(root),'foreign production control'
    for part in [root,*list(path.relative_to(root).parents)[:-1]]:
        candidate=part if part==root else root/part
        assert not candidate.is_symlink() and not (candidate.stat().st_file_attributes & 0x400 if hasattr(candidate.stat(),'st_file_attributes') else False),'linked production directory'
    assert not path.is_symlink() and not (path.stat().st_file_attributes & 0x400 if hasattr(path.stat(),'st_file_attributes') else False),'linked production file'
    assert path.resolve().is_relative_to(root.resolve()),'foreign production file'

def production_stat(info):
    return (info.st_dev,info.st_ino,info.st_mode,info.st_size,info.st_mtime_ns,info.st_ctime_ns)

def production_snapshot_stable(snapshot):
    before,after,current,_,initial=snapshot
    # Windows path ctime is creation time; handle ctime is change time (Python 3.12).
    # Preserve both providers' full metadata; only join fields with common semantics.
    return before==after and initial==current and before[:5]==current[:5]

def production_read(path,root):
    production_file(path,root);initial=production_stat(path.lstat())
    assert stat.S_ISREG(initial[2]) and initial[3]<=4096,'production control must be a regular file <=4KiB'
    def unique(pairs):
        result={}
        for key,value in pairs:
            assert key not in result,'duplicate production control key'
            result[key]=value
        return result
    # Value, identity and digest are taken from this one bounded open file.
    with path.open('rb') as control:
        before=production_stat(os.fstat(control.fileno()))
        assert stat.S_ISREG(before[2]) and before[3]<=4096,'production control must be a regular file <=4KiB'
        raw=control.read(4097);assert len(raw)<=4096,'production control cap'
        error=None
        try:value=json.loads(raw,object_pairs_hook=unique)
        except json.JSONDecodeError as exc:error=exc
        after=production_stat(os.fstat(control.fileno()))
        production_file(path,root);current=production_stat(path.lstat())
        assert initial[:3]==before[:3]==after[:3]==current[:3],'production control replaced during read'
        assert after[3]<=4096 and current[3]<=4096,'production control cap'
        snapshot=(before,after,current,hashlib.sha256(raw).hexdigest(),initial)
        if production_snapshot_stable(snapshot):assert len(raw)==after[3],'production control byte count changed'
    if error is not None:
        error.production_snapshot=snapshot
        raise error
    return value,snapshot

def production_remember(path,root,value,snapshot,controls):
    production_file(path,root)
    assert production_snapshot_stable(snapshot) and snapshot[2]==production_stat(path.lstat()),'production control changed before observation'
    assert path not in controls or controls[path]==snapshot,'production control replaced'
    controls[path]=snapshot
    return value

def production_control(path,root,controls=None):
    value,snapshot=production_read(path,root)
    return production_remember(path,root,value,snapshot,{} if controls is None else controls)

def incomplete_owned_json(error):
    text=error.doc.rstrip()
    return error.pos>=len(text) or (error.msg.startswith('Unterminated string') and not text.endswith(('}',']')))

def initial_owned_snapshot(state,snapshot):
    identity=snapshot[0][:3]
    assert state.setdefault('initialOwnedIdentity',identity)==identity,'initial production ownership control replaced'
    assert time.monotonic()-state['initialOwnedAt']<2,'initial ownership publication deadline'
    return production_snapshot_stable(snapshot)

def production_payload(data,expected):
    return type(data) is dict and set(data)==set(expected) and all(type(data[k]) is type(v) and data[k]==v for k,v in expected.items())

def production_request(root,data):
    w,h,nonce,profile=production_tuple(root)
    expected=dict(width=w,height=h,nonce=nonce,animated=False,privateProfile='borderless' if profile=='popup' else profile)
    assert production_payload(data,expected),'production static tuple/profile mismatch'

def complete_production_raw(path,root):
    if not path.exists():return False
    production_file(path,root);size=path.stat().st_size
    assert size<=9*1024*1024,'production PAM cap'
    with path.open('rb') as source:header=source.read(256)
    end=header.find(b'ENDHDR\n')
    if end<0:
        assert size<256,'invalid production PAM header'
        return False
    header=header[:end+7]
    match=re.fullmatch(rb'P7\nWIDTH ([0-9]+)\nHEIGHT ([0-9]+)\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n',header)
    assert match,'invalid production PAM header'
    w,h=map(int,match.groups());assert 0<w<=1920 and 0<h<=1080,'production PAM dimensions'
    expected=len(header)+w*h*4
    assert size<=expected,'extra production PAM bytes'
    return size==expected

def poll_production_static(root,driver_pid,sources,opened,state):
    now=time.monotonic();w,h,nonce,profile=production_tuple(root)
    source=root/'source';assert all(p.name=='source-command-1.json' for p in source.glob('source-command-*.json')),'unexpected production source command'
    assert len(sources.children)<=1 and set(sources.children)<={source},'unexpected production source owner'
    if not sources.children:
        assert now-state.get('driverStartedAt',now)<5,'production source request deadline'
        return
    item=sources.children[source];proc=item['proc']
    assert now-item.get('launchedAt',item.get('readyAt',now))<38 or state.get('phase')=='postcapture' or (root/'production-phase.json').exists(),'production source lifetime watchdog'
    if not item['observed']:
        assert proc.poll() is None,'production source died before observation'
        assert now-item.get('launchedAt',now)<5,'production source observation deadline'
        return
    def control(path):
        return production_control(path,root,state.setdefault('controls',{}))
    production_request(root,control(source/'source-request.json'))
    ready=control(source/'source-ready.json');observed=control(source/'source-observed.json')
    assert production_payload(observed,ready) and set(ready)=={'pid','hwnd'} and all(type(ready[k]) is int and ready[k]>0 for k in ready),'wrong production source identity'
    assert ready['pid']==proc.pid!=driver_pid,'wrong production source PID'
    identity=dict(hwnd=ready['hwnd'],sourcePid=proc.pid)
    if 'identity' not in state:state.update(identity=identity,sourceReadyAt=item['readyAt'],phase='source')
    assert state['identity']==identity,'production owner changed'
    phase=root/'production-phase.json'
    if phase.exists():
        value=control(phase)
        assert production_payload(value,dict(phase='postcapture',**identity,workerEncoderJoined=True,sourceClosed=True,qpc=value.get('qpc'))) and type(value['qpc']) is int and value['qpc']>0,'invalid postcapture phase'
        assert state.get('holdCompleteAt') is not None and len(opened)==1 and all(handle.signaled() for _,handles in opened.values() for handle in handles),'postcapture without joined exact owners'
        assert proc.poll()==0 and production_payload(control(source/'source-exit.json'),dict(pid=proc.pid,exitCode=0)),'postcapture source not normally closed'
        state['phase']='postcapture'
        return
    assert now-state['sourceReadyAt']<38,'production capture-phase watchdog'
    # close() writes the command after finish and waits for this owner to observe source exit.
    closing=source/'source-command-1.json'
    if closing.exists():
        assert state.get('holdCompleteAt') is not None and len(opened)==1 and all(handle.signaled() for _,handles in opened.values() for handle in handles),'premature production source close'
        assert production_payload(control(closing),dict(sequence=1,action='close')),'unexpected production source command'
        assert proc.poll() in (None,0),'production source close failed'
        state['phase']='closing'
        return
    assert proc.poll() is None,'production source exited before finish'
    assert window_pid(identity['hwnd'])==proc.pid,'production HWND changed owner'
    selected=source/'source-selection.json'
    if not selected.exists():return
    assert production_payload(control(selected),dict(driverPid=driver_pid,**identity)),'production source selection mismatch'
    start=root/'production-start-authorized.json';complete=root/'production-actions-complete.json'
    if state['phase']=='source':
        assert not start.exists() and not start.is_symlink() and not complete.exists() and not complete.is_symlink(),'stale production authorization'
        marker=root/'production-source-ready.json'
        if not marker.exists():return
        assert production_payload(control(marker),identity),'production source-ready mismatch'
        fixture_write(start,dict(action='static-owned-canvas-v1',**identity));state.update(phase='authorized',authorizedAt=now)
    assert production_payload(control(start),dict(action='static-owned-canvas-v1',**identity)),'malformed production start authorization'
    if 'armedAt' not in state:assert now-state['authorizedAt']<15,'production prepare/arm deadline'
    armed=root/'production-armed.json'
    if armed.exists():
        assert production_payload(control(armed),dict(**identity,profile='pmv2_96_single_monitor_'+profile,candidateOnly=True)),'production armed identity/profile mismatch'
        owned=root/'session-0/owned.json'
        assert set(opened)=={owned},'production armed before exact ownership observation'
        data,handles=opened[owned]
        assert control(owned)==data and set(data)=={'parent','worker','encoder','qpc'} and data['parent']==driver_pid and len(handles)==2,'production owned identity changed'
        assert all(type(data[k]) is int and data[k]>0 for k in ['parent','worker','encoder']) and len({data[k] for k in ['parent','worker','encoder']}|{proc.pid})==4,'invalid production owner tuple'
        state.setdefault('armedAt',now)
        if 'holdCompleteAt' not in state:
            assert not complete.exists() and not complete.is_symlink(),'stale production completion'
            assert now-state['armedAt']<3,'production static hold deadline'
            if now-state['armedAt']>=.5 and all(complete_production_raw(root/f'session-0/raw-{i}.pam',root) for i in [0,1]):
                fixture_write(complete,dict(action='static-hold-complete-v1',**identity));state.update(phase='held',holdCompleteAt=now)
    if 'holdCompleteAt' in state:
        assert production_payload(control(complete),dict(action='static-hold-complete-v1',**identity)),'malformed production hold authorization'
    else:assert not complete.exists() and not complete.is_symlink(),'premature production completion'

class GeneratedSources:
    def __init__(self,harness,root,env,production_static=False):self.harness=harness;self.root=root;self.env=env;self.children={};self.production_static=production_static;self.controls={}
    def poll(self,driver_pid):
        for request in ([self.root/'source/source-request.json'] if self.production_static and (self.root/'source/source-request.json').exists() else [] if self.production_static else self.root.glob('session-*/source-request.json')):
            case=request.parent
            if case in self.children:continue
            assert not case.is_symlink() and case.name in ({'source'} if self.production_static else {'session-0','session-1','session-2'}) and len(self.children)<(1 if self.production_static else 3),'unexpected source session'
            data=production_control(request,self.root,self.controls) if self.production_static else fixture_read(request)
            if self.production_static:production_request(self.root,data)
            private='privateProfile' in data
            if private:
                if data.get('privateProfile')=='startup-metadata':require_metadata_disposable_context()
                else:require_cursor_disposable_context()
            assert set(data)=={'width','height','nonce','animated'}|({'privateProfile'} if private else set())
            if private:assert data['privateProfile'] in {'borderless','decorated','startup-metadata'} and data['animated'] is False
            if data.get('privateProfile')=='startup-metadata':assert [data['width'],data['height'],data['nonce']]==[320,240,601]
            assert type(data['width']) is int and 160<=data['width']<=1920
            assert type(data['height']) is int and 120<=data['height']<=1080
            assert type(data['nonce']) is int and 0<=data['nonce']<=65535 and type(data['animated']) is bool
            argv=[str(self.harness),'--source',str(case),str(data['width']),str(data['height']),str(data['nonce']),str(int(data['animated']))]
            if private:argv[1]='--source-private';argv.append(data['privateProfile'])
            with (case/'source.log').open('wb') as out:
                proc=subprocess.Popen(argv,stdin=subprocess.DEVNULL,stdout=out,stderr=subprocess.STDOUT,env=self.env)
            # Retained Popen owns the exact process handle even before the HWND handshake succeeds.
            self.children[case]=dict(proc=proc,argv=argv,observed=False,launchedAt=time.monotonic(),aliveBeforeParentKillQpc=None,aliveAfterProductExitQpc=None)
        for case,item in self.children.items():
            proc=item['proc'];code=proc.poll()
            if code is not None:
                if not (case/'source-exit.json').exists():fixture_write(case/'source-exit.json',dict(pid=proc.pid,exitCode=code))
                assert code==0 and any(fixture_read(p).get('action')=='close' for p in case.glob('source-command-*.json')),'source exited unexpectedly'
                continue
            ready=case/'source-ready.json'
            if not item['observed'] and ready.is_file():
                data=production_control(ready,self.root,self.controls) if self.production_static else fixture_read(ready)
                assert set(data)=={'pid','hwnd'} and type(data['pid']) is int and type(data['hwnd']) is int
                assert data['pid']==proc.pid!=driver_pid and data['hwnd']>0,'wrong source process identity'
                assert window_pid(data['hwnd'])==proc.pid,'HWND does not belong to exact source process'
                fixture_write(case/'source-observed.json',data);item['observed']=True;item['readyAt']=time.monotonic()
    def require_alive(self,stage=None):
        assert self.children and all(x['observed'] and x['proc'].poll() is None for x in self.children.values()),'source died before product Job observation'
        assert not any(fixture_read(p).get('action')=='close' for case in self.children for p in case.glob('source-command-*.json')),'source close preceded product Job observation'
        if stage:
            assert stage in {'aliveBeforeParentKillQpc','aliveAfterProductExitQpc'}
            for item in self.children.values():item[stage]=native_qpc()
    def cleanup(self,error,parent_death=False):
        failures=[]
        for case,item in self.children.items():
            proc=item['proc'];fallback=False;cleanup_error=None
            try:
                if error or parent_death:
                    if proc.poll() is None:proc.kill()
                else:
                    try:proc.wait(timeout=2)
                    except subprocess.TimeoutExpired:fallback=True;proc.kill()
                proc.wait(timeout=3)
                if not error and not parent_death:assert proc.returncode==0 and not fallback,'source required fallback cleanup'
            except BaseException as exc:cleanup_error=repr(exc);failures.append(cleanup_error)
            finally:
                try:fixture_write(case/'source-cleanup.json',dict(pid=proc.pid,argv=item['argv'],observed=item['observed'],exitCode=proc.poll(),signaled=proc.poll() is not None,aliveBeforeParentKillQpc=item['aliveBeforeParentKillQpc'],aliveAfterProductExitQpc=item['aliveAfterProductExitQpc'],fallback=fallback,error=cleanup_error,reason='error' if error else 'after-product-job-observation' if parent_death else 'driver-close'))
                except BaseException as exc:failures.append(repr(exc))
        assert not failures,failures

def execute(argv,root,name,timeout=600,env=None,observe=None,parent_death=False,production_static=False):
    if name=='startup-metadata':require_metadata_disposable_context()
    if production_static:
        require_cursor_disposable_context();assert observe is not None and not parent_death and timeout<=120
        production_tuple(observe)
    started=time.monotonic();opened={};media={};killed_at=None;kill_qpc=None;receipts=[];error=None;error_filename=None;error_traceback=None
    log=root/(name+'.log');receipt=root/(name+'-command.json')
    sources=GeneratedSources(argv[0],observe,env,production_static=production_static) if observe else None
    production_state=dict(driverStartedAt=started,deadlines=dict(driverSeconds=timeout,sourceObservationSeconds=5,capturePhaseSeconds=38,prepareArmSeconds=15,holdSeconds=3,minimumHoldMs=500,initialOwnedSeconds=2),controls=sources.controls) if production_static else {}
    with log.open('wb') as out:
        proc=subprocess.Popen([str(x) for x in argv],stdout=out,stderr=subprocess.STDOUT,env=env)
        try:
            while proc.poll() is None:
                bound(root,production_parent=(observe.name,proc.pid)) if production_static else bound(root)
                assert time.monotonic()-started<timeout,'outer watchdog fired; never product success'
                if observe:
                    sources.poll(proc.pid)
                    ownership_paths=list(observe.glob('session-*/owned.json'))
                    ownership_paths+=list(observe.glob('session-*/without/owned.json'))+list(observe.glob('session-*/with/owned.json'))+list(observe.glob('session-*/alpha-without/owned.json'))+list(observe.glob('session-*/alpha-with/owned.json'))+list(observe.glob('session-*/challenge/owned.json'))+list(observe.glob('session-*/resize/owned.json'))+list(observe.glob('session-*/closed/owned.json'))
                    ownership_paths+=list(observe.glob('session-0/startup-probe/metadata-owned.json'))
                    for path in ownership_paths:
                        if path in opened:continue
                        if production_static:
                            assert path==observe/'session-0/owned.json','unexpected production ownership layout'
                            production_state.setdefault('initialOwnedAt',time.monotonic())
                            try:data,snapshot=production_read(path,observe)
                            except json.JSONDecodeError as exc:
                                if not incomplete_owned_json(exc):raise
                                initial_owned_snapshot(production_state,exc.production_snapshot)
                                continue
                            assert type(data) is dict and set(data)=={'parent','worker','encoder','qpc'} and all(type(data[k]) is int and data[k]>0 for k in ['parent','worker','encoder']) and type(data['qpc']) is int and data['qpc']>=0,'invalid production ownership tuple'
                            assert data['parent']==proc.pid,'foreign production ownership parent'
                            sources.require_alive()
                            assert len({data[k] for k in ['parent','worker','encoder']}|{item['proc'].pid for item in sources.children.values()})==4,'production owner identities overlap'
                            if not initial_owned_snapshot(production_state,snapshot):continue
                            production_remember(path,observe,data,snapshot,production_state['controls'])
                            assert not path.with_name('handles-open').exists() and not path.with_name('handles-open').is_symlink(),'stale production exact-handle gate'
                        else:
                            try:data=json.loads(path.read_text(encoding='utf-8'))
                            except (OSError,ValueError):continue
                        metadata=path.name=='metadata-owned.json'
                        if metadata:
                            require_metadata_disposable_context();assert name=='startup-metadata' and set(data)=={'parent','worker','qpc'},'unexpected metadata ownership receipt'
                        assert data['parent']==proc.pid
                        handles=[]
                        try:
                            for kind in (['worker'] if metadata else ['worker','encoder']):handles.append(ExactHandle(data[kind]))
                        except BaseException:
                            for handle in handles:handle.close()
                            raise
                        opened[path]=(data,handles)
                        path.with_name('metadata-handles-open' if metadata else 'handles-open').write_text('independently held exact noninheritable process handles\n',encoding='utf-8')
                    if production_static:poll_production_static(observe,proc.pid,sources,opened,production_state)
                    for gate in observe.glob('session-*/before-publish'):
                        if gate not in media:
                            media[gate]=sha(gate.with_name('screen.mp4'))
                            gate.with_name('media-hashed').write_text('independent SHA256 retained before publication\n',encoding='utf-8')
                    if parent_death and opened and any(p.with_name('parked').is_file() for p in opened):
                        assert killed_at is None
                        sources.require_alive('aliveBeforeParentKillQpc')
                        assert all(not h.signaled() for _,handles in opened.values() for h in handles), 'child died before parent kill'
                        kill_qpc=native_qpc();killed_at=time.monotonic();proc.kill()
                time.sleep(.005)
            code=proc.wait(timeout=3)
            if parent_death:assert killed_at is not None,'parent died before isolation gate'
            if production_static:
                sources.poll(proc.pid);poll_production_static(observe,proc.pid,sources,opened,production_state)
                assert production_state.get('phase')=='postcapture' and len(media)==1,'production did not complete postcapture/hash handshake'
            for path,(data,handles) in opened.items():
                deadline=(killed_at if parent_death else time.monotonic())+2
                while not all(h.signaled() for h in handles) and time.monotonic()<deadline:time.sleep(.005)
                signaled=[h.signaled() for h in handles]
                item=dict(**data,signaled=signaled,observer='independent Python exact process handles',parentDeath=parent_death,aliveImmediatelyBeforeParentKill=parent_death,killQpc=kill_qpc,observedSignaledQpc=native_qpc(),afterParentKillMs=None if killed_at is None else (time.monotonic()-killed_at)*1000,fallback=False)
                if not all(signaled):
                    item['fallback']=True
                    for h in handles:
                        if not h.signaled():h.kill()
                receipts.append(item)
                assert all(signaled),'exact handles not signaled before fallback/deadline'
            if parent_death:sources.require_alive('aliveAfterProductExitQpc')
            if observe and not any(x in name for x in ['admission','marker-failure']):assert opened,'no independently held child handles'
            for gate,before in media.items():
                after=sha(gate.with_name('screen.mp4'));write(gate.with_name('media-preservation.json'),dict(beforePublicationSha256=before,afterImportSha256=after,unchanged=before==after))
                assert before==after,'publication/import changed media bytes'
            if not parent_death:assert code==0,f'command exit {code}'
        except BaseException as exc:
            error=repr(exc);error_filename=str(exc.filename)[:1024] if isinstance(exc,OSError) and exc.filename is not None else None;error_traceback=traceback.format_exc(limit=8)[-8192:]
            if proc.poll() is None:proc.kill();proc.wait(timeout=5)
        finally:
            if sources:
                try:sources.cleanup(error is not None,parent_death=parent_death and error is None)
                except BaseException as exc:
                    if error is None:error=repr(exc);error_filename=str(exc.filename)[:1024] if isinstance(exc,OSError) and exc.filename is not None else None;error_traceback=traceback.format_exc(limit=8)[-8192:]
            cleanup=[];deadline=time.monotonic()+2
            for path,(data,handles) in opened.items():
                signaled=[]
                for h in handles:
                    try:
                        if not h.signaled():h.kill()
                        while not h.signaled() and time.monotonic()<deadline:time.sleep(.005)
                        signaled.append(h.signaled())
                    except BaseException as exc:
                        signaled.append(None)
                        if error is None:error=repr(exc);error_traceback=traceback.format_exc(limit=8)[-8192:]
                    finally:
                        try:h.close()
                        except BaseException as exc:
                            signaled[-1]=None
                            if error is None:error=repr(exc);error_traceback=traceback.format_exc(limit=8)[-8192:]
                cleanup.append(dict(**data,path=path.relative_to(observe).as_posix(),signaled=signaled))
            final=dict(parent=dict(pid=proc.pid,signaled=proc.poll() is not None,exitCode=proc.returncode),owned=cleanup,sources=[dict(pid=item['proc'].pid,observed=item['observed'],signaled=item['proc'].poll() is not None,exitCode=item['proc'].poll()) for item in sources.children.values()] if sources else [])
            write(receipt,dict(argv=[str(x) for x in argv],exitCode=proc.returncode,seconds=time.monotonic()-started,error=error,errorFilename=error_filename,errorTraceback=error_traceback,logSha256=sha(log),ownership=receipts,cleanup=final,productionStaticPhase={k:v for k,v in production_state.items() if k!='controls'} if production_static else None,cleanupUnknown=bool(production_static and error)))
    if error:raise RuntimeError(error)
    return log

def require_private_cleanup(root,receipt,complete=False):
    data=json.loads(receipt.read_text(encoding='utf-8'));cleanup=data['cleanup'];parent=cleanup['parent']
    assert parent['signaled'] is True and type(parent['pid']) is int and parent['pid']>0 and type(parent['exitCode']) is int,'private parent cleanup unconfirmed'
    allowed={f'session-0/{mode}/owned.json' for mode in ['without','with','alpha-without','alpha-with','challenge','resize','closed']}
    paths={p.relative_to(root).as_posix():p for p in root.rglob('owned.json')}
    assert all((root/p).is_file() for p in allowed if (root/p).parent.exists()),'private proof started without observed owner'
    owned=cleanup['owned'];assert paths and set(paths)<=allowed and len(owned)==len(paths),'unknown/unobserved private owner'
    assert {row['path'] for row in owned}==set(paths),'private ownership cleanup incomplete'
    pids={parent['pid']}
    for row in owned:
        original=json.loads(paths[row['path']].read_text(encoding='utf-8'))
        assert set(original)=={'parent','worker','encoder','qpc'} and all(row[k]==original[k] for k in original),'private owner identity changed'
        assert row['parent']==parent['pid'] and row['signaled']==[True,True] and all(value is True for value in row['signaled']),'private worker/encoder cleanup unconfirmed'
        for key in ['worker','encoder']:
            assert type(row[key]) is int and row[key]>0 and row[key]!=parent['pid'],'invalid exact private owner'
            pids.add(row[key])
    sources=cleanup['sources'];assert len(sources)==1,'private source ownership unknown'
    assert {p.relative_to(root).as_posix() for p in root.rglob('source-cleanup.json')}=={'session-0/source-cleanup.json'},'unexpected private source owner'
    source=sources[0];saved=json.loads((root/'session-0/source-cleanup.json').read_text(encoding='utf-8'))
    assert type(source['pid']) is int and source['pid']>0 and source['pid'] not in pids and source['observed'] is True and source['signaled'] is True and type(source['exitCode']) is int,'private source cleanup unconfirmed'
    assert all(saved[k]==source[k] for k in ['pid','observed','signaled','exitCode']),'private source cleanup identity disagrees'
    if complete:
        assert set(paths)==allowed and len(pids)==15,'successful private profile requires seven distinct named worker/encoder pairs'
        assert data['error'] is None and data['exitCode']==parent['exitCode']==source['exitCode']==0,'successful private profile did not exit normally'
        observations=data['ownership'];keys=['parent','worker','encoder','qpc']
        identities={tuple(row[k] for k in keys) for row in owned}
        observed=[tuple(row[k] for k in keys) for row in observations]
        assert len(observed)==len(set(observed))==7 and set(observed)==identities,'successful private profile lacks seven matching independent owner observations'
        assert all(type(row['qpc']) is int and row['qpc']>=0 and row['signaled']==[True,True] and all(value is True for value in row['signaled']) and row['fallback'] is False for row in observations),'successful private owner observation lacks normal exact-handle completion'

def require_production_cleanup(root,receipt,complete=False)->dict:
    data=json.loads(receipt.read_text(encoding='utf-8'));cleanup=data['cleanup'];parent=cleanup['parent']
    assert not data.get('cleanupUnknown',False),'production descendant cleanup unknown; abort lane'
    assert parent['signaled'] is True and type(parent['pid']) is int and parent['pid']>0 and type(parent['exitCode']) is int,'production parent cleanup unconfirmed'
    assert {p.relative_to(root).as_posix() for p in root.rglob('owned.json')}=={'session-0/owned.json'},'unknown production owner'
    rows=cleanup['owned'];assert len(rows)==1 and rows[0]['path']=='session-0/owned.json','production exact owner missing'
    row=rows[0];original=production_control(root/'session-0/owned.json',root)
    assert set(original)=={'parent','worker','encoder','qpc'} and all(row[k]==original[k] for k in original),'production owner changed'
    assert row['parent']==parent['pid'] and row['signaled']==[True,True] and all(x is True for x in row['signaled']),'production worker/encoder cleanup unconfirmed'
    sources=cleanup['sources'];assert len(sources)==1,'production source owner missing'
    assert {p.relative_to(root).as_posix() for p in root.rglob('source-cleanup.json')}=={'source/source-cleanup.json'},'unexpected production source cleanup'
    source=sources[0];saved=production_control(root/'source/source-cleanup.json',root)
    assert source['observed'] is True and source['signaled'] is True and type(source['exitCode']) is int and saved['fallback'] is False and saved['error'] is None,'production source cleanup unconfirmed'
    assert all(saved[k]==source[k] for k in ['pid','observed','signaled','exitCode']),'production source identity changed'
    ids=[parent['pid'],row['worker'],row['encoder'],source['pid']]
    assert all(type(pid) is int and pid>0 for pid in ids) and len(set(ids))==4,'invalid production owner identities'
    if complete:
        assert data['error'] is None and data['exitCode']==parent['exitCode']==source['exitCode']==0,'production did not exit normally'
        observed=data['ownership'];assert len(observed)==1 and all(observed[0][k]==original[k] for k in original),'production independent owner observation missing'
        assert observed[0]['signaled']==[True,True] and all(x is True for x in observed[0]['signaled']) and observed[0]['fallback'] is False,'production exact handle fallback'
        assert data['productionStaticPhase']['phase']=='postcapture','production postcapture phase missing'
    return saved

def build_exe(repo,evidence,manifest,name,test=False):
    args=['cargo','+1.89.0','test' if test else 'build','--locked','--manifest-path',manifest,'--message-format=json','--test' if test else '--bin',name]
    if test:args.append('--no-run')
    log=execute(args,evidence,'build-'+name,1200)
    artifacts=[]
    for line in log.read_text(encoding='utf-8').splitlines():
        try:item=json.loads(line)
        except ValueError:continue
        if item.get('reason')=='compiler-artifact' and item.get('target',{}).get('name')==name and item.get('executable'):artifacts.append(Path(item['executable']))
    assert len(set(artifacts))==1,f'no unique compiled executable for {name}'
    return artifacts[0].resolve()

def uninstrumented(case,ffmpeg,ffprobe):
    manifest=json.loads((case/'capture.json').read_text(encoding='utf-8'));n=manifest['state']['videoTiming']['clock']['completedFrames'];w=manifest['source']['rawSize']['width'];h=manifest['source']['rawSize']['height']
    meta=json.loads(subprocess.check_output([str(ffprobe),'-v','error','-show_streams','-show_packets','-of','json',str(case/'screen.mp4')],timeout=20,encoding='utf-8'));assert len(meta['streams'])==1
    stream=meta['streams'][0];assert stream['codec_name']=='h264' and stream['pix_fmt']=='yuv420p';analyze.packets(meta['packets'],stream['time_base'],n)
    paints=set(tuple(x) for x in json.loads((case/'paint-history.json').read_text(encoding='utf-8')));ew,eh=stream['width'],stream['height']
    assert (ew,eh)==(w+w%2,h+h%2),'wrong uninstrumented geometry'
    proc=subprocess.Popen([str(ffmpeg),'-v','error','-i',str(case/'screen.mp4'),'-f','rawvideo','-pix_fmt','rgb24','pipe:1'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
    import threading
    timer=threading.Timer(20,proc.kill);timer.start();ids=[]
    try:
        for _ in range(n):
            paint,nonce,corners=analyze.identity(proc.stdout.read(ew*eh*3),ew,eh,w,h);assert (paint,nonce) in paints;ids.append([paint,nonce,corners])
        assert proc.stdout.read(1)==b'';assert proc.wait(timeout=5)==0
    finally:
        timer.cancel()
        if proc.poll() is None:proc.kill();proc.wait(timeout=5)
    write(case/'analysis.json',dict(instrumented=False,frames=n,decoded=ids,packets=meta['packets'],videoSha256=sha(case/'screen.mp4'),limits='Visual/grid/import smoke; no causal callback trace in actual release worker.'))

def run(a):
    assert sys.platform=='win32','native lane must run on Windows'
    scope=getattr(a,'scope','full');assert scope in {'full','close-diagnostic','cursor-private','startup-metadata','cursor-production-static'},'unknown qualification scope'
    cursor_private=scope=='cursor-private';startup_metadata=scope=='startup-metadata';production_static=scope=='cursor-production-static'
    disposable=require_metadata_disposable_context() if startup_metadata else require_cursor_disposable_context() if cursor_private or production_static else None
    diagnostic=scope=='close-diagnostic'
    expected_branch='codex-windows-startup-metadata' if startup_metadata else 'codex-windows-cursor-qualification' if cursor_private or production_static else 'codex-windows-owned-close-diagnostic' if diagnostic else 'codex-windows-owned-product-qualification'
    observed_ref=os.getenv('GITHUB_REF')
    assert observed_ref is None or observed_ref=='refs/heads/'+expected_branch,'qualification scope/ref mismatch'
    if diagnostic:print('DIAGNOSTIC SUBSET: close only; cannot establish full qualification PASS')
    repo=Path(__file__).resolve().parents[2];os.chdir(repo)
    if cursor_private or production_static or startup_metadata:assert subprocess.check_output(['git','rev-parse','HEAD'],text=True,encoding='ascii').strip()==disposable['sha'],'checkout/run SHA mismatch before native tools'
    evidence=a.evidence.resolve();evidence.mkdir();assert sha(a.ffmpeg)==FFMPEG_SHA and sha(a.ffprobe)==FFPROBE_SHA
    before=source_manifest(repo);write(evidence/'source-manifest.json',before)
    write(evidence/'provenance.json',dict(scope=scope,privateCursorProof=cursor_private,productionStatic=production_static,checkerHashes={name:sha(repo/"tools/windows-owned-product-qualification"/name) for name in ["production_cursor_check.py","cursor_check.py","analyze.py","package_evidence.py","run.py"]},disposableContext=disposable,diagnosticOnly=diagnostic or startup_metadata,metadataOnly=startup_metadata,expectedBranch=expected_branch,observedRef=observed_ref,commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True,encoding='ascii').strip(),runnerImage=os.getenv('ImageVersion'),runnerOS=os.getenv('RUNNER_OS'),ffmpegSha256=sha(a.ffmpeg),ffprobeSha256=sha(a.ffprobe),archiveSha256=ARCHIVE_SHA,python=sys.version,platform=sys.platform,toolchain='1.89.0',profile='scalar metadata only; no encoder/ARM/admission' if startup_metadata else 'debug; H264 libx264 veryfast CRF23 yuv420p B0 30fps 30000-timescale',environment={k:os.getenv(k) for k in ['CARGO_TARGET_DIR','VCToolsVersion','WindowsSDKVersion','PROCESSOR_IDENTIFIER']}))
    metadata=json.loads(subprocess.check_output(['cargo','+1.89.0','metadata','--locked','--manifest-path','FocusFlow/src-tauri/Cargo.toml','--format-version','1'],text=True,encoding='utf-8'))
    vendor=[x for x in metadata['packages'] if x['name']=='windows-capture'];assert len(vendor)==1 and vendor[0]['source'] is None
    assert Path(vendor[0]['manifest_path']).resolve()==repo/'FocusFlow/vendor/windows-capture/Cargo.toml'
    write(evidence/'dependency-resolution.json',metadata)
    failures=[]
    def attempt(name,fn):
        try:fn()
        except Exception as error:failures.append(dict(name=name,error=repr(error)));write(evidence/'failures.json',failures)
    app='FocusFlow/src-tauri/Cargo.toml';core='FocusFlow/crates/focusflow-core/Cargo.toml';mapping='.superpowers/sdd/2026-10-27-windows-owned-product/fix-1-native-harness/Cargo.toml'
    # Native file sync, actual mapping/debug layer, and Windows pipes are distinct required gates.
    def require_tests(argv,name,count,names=()):
        log=execute(argv,evidence,name)
        require_test_count(log.read_text(encoding='utf-8',errors='replace'),count,names)
    if not diagnostic and not cursor_private and not startup_metadata and not production_static:
        attempt('core-publication',lambda:require_tests(['cargo','+1.89.0','test','--locked','--manifest-path',core,'--lib','capture::windows::tests::','--','--test-threads=1','--nocapture'],'core-publication',10))
        attempt('core-import',lambda:require_tests(['cargo','+1.89.0','test','--locked','--manifest-path',core,'--test','windows_capture','--','--test-threads=1','--nocapture','--skip','common::child_writer'],'core-import',WINDOWS_IMPORT_TEST_COUNT))
        attempt('mapping-debug',lambda:require_tests(['cargo','+1.89.0','test','--locked','--manifest-path',mapping,'--lib','frame::mapping_tests::','--','--test-threads=1','--nocapture'],'mapping-debug',4))
    def pipes():
        exe=build_exe(repo,evidence,core,'cfr_grid_windows',True);env=os.environ.copy();env['CORE_QUAL_EVIDENCE']=str(evidence/'core-pipes')
        execute([exe],evidence,'core-pipes',90,env)
    if not diagnostic and not cursor_private and not startup_metadata and not production_static:attempt('core-pipes',pipes)
    if cursor_private:attempt('cursor-renderer',lambda:require_tests(['cargo','+1.89.0','test','--locked','--manifest-path',core,'--test','cursor_render','lossless_rgba_compositing_isolates_alpha_and_hotspot','--','--test-threads=1'],'cursor-renderer',1))
    if cursor_private:attempt('cursor-portable',lambda:require_tests(['cargo','+1.89.0','test','--locked','--manifest-path',core,'--test','windows_cursor_private','--','--test-threads=1','--nocapture','--skip','common::child_writer'],'cursor-portable',6))
    if cursor_private:
        for label,filter_name,count,names in [
            ('cursor-startup-lifecycle','recorder::windows_capture::lifecycle::tests::',11,[
                'recorder::windows_capture::lifecycle::tests::'+name for name in [
                    'metadata_only_until_arm_and_terminal_is_absorbing','queued_gates_cannot_override_eof_or_early_stop',
                    'real_startup_sequence_injects_faults_at_reachable_owned_operations','real_startup_sequence_has_one_encoder_and_admits_only_after_park_and_writer',
                    'feed_ack_precedes_finish_when_reader_pauses_before_stop_publication','feed_end_never_acknowledges_startup_cancellation_or_invents_automatic_stop']]),
            ('cursor-startup-contract','recorder::windows_capture::tests::',11,[
                'recorder::windows_capture::tests::'+name for name in [
                    'terminal_rejects_partial_killed_missing_and_inconsistent_results','frozen_media_dimensions_stay_independent_of_observed_bounds_and_legacy_origin',
                    'automatic_end_accepts_stop_during_finalization_without_inventing_an_ack']]),
            ('cursor-startup-gate','recorder::owned_feed::tests::',9,[
                'recorder::owned_feed::tests::'+name for name in [
                    'startup_geometry_freezes_first_equal_callback_without_pixels','startup_geometry_deadline_invalid_and_postfreeze_changes_fail','startup_gate_action_is_not_upgraded_by_later_arm']])]:
            attempt(label,lambda label=label,filter_name=filter_name,count=count,names=names:require_tests(['cargo','+1.89.0','test','--locked','--manifest-path',app,'--lib',filter_name,'--','--test-threads=1','--nocapture'],label,count,names))
    def product():
        if cursor_private and failures:return
        built={}
        targets=[('windows_owned_qualification',True)] if diagnostic or cursor_private or production_static or startup_metadata else [('FocusFlow',False),('windows_owned_qualification',True),('windows_encoder_fixture',True)]
        for name,test in targets:
            attempt('build-'+name,lambda name=name,test=test:built.update({name:build_exe(repo,evidence,app,name,test)}))
        actual=built.get('FocusFlow');harness=built.get('windows_owned_qualification');fixture=built.get('windows_encoder_fixture')
        write(evidence/'executables.json',{str(p):sha(p) for p in built.values()})
        assert harness is not None,'instrumented harness unavailable; dependent native cases not run'
        if startup_metadata:
            import startup_check
            def metadata_case():
                root=evidence/'startup-metadata';execute([harness,'--startup-metadata',root,harness],evidence,'startup-metadata',30,observe=root)
                startup_check.check(root/'session-0',evidence/'startup-metadata-command.json')
            attempt('startup-metadata',metadata_case);return
        if not diagnostic and not cursor_private and not startup_metadata and not production_static:attempt('native-platform',lambda:execute([harness,'--platform',evidence/'native-platform.json'],evidence,'native-platform',20))
        def one(label,mode,w,h,nonce,worker,encoder):
            root=evidence/label
            execute([harness,root,mode,w,h,nonce,encoder,worker],evidence,label,45,observe=root,parent_death=mode=='parent-death')
            if mode in ['blocked','partial','exit7','lost-worker','parent-death','transfer-shutdown','admission-close','admission-resize','marker-failure']:
                if mode=='blocked':
                    t=json.loads((root/'session-0/trace.json').read_text(encoding='utf-8'));assert t['supervisorTerminated'] is True and any(not x['ok'] for x in t['writes'])
                if mode=='partial':
                    prefix=json.loads((root/'session-0/consumer-prefix.json').read_text(encoding='utf-8'));assert prefix['consumerBytes']==65536
                if mode=='exit7':require_exit7(root/'session-0')
                return
            for case in root.glob('session-*'):
                if worker==actual:uninstrumented(case,a.ffmpeg,a.ffprobe)
                else:analyze.analyze(case,a.ffmpeg,a.ffprobe)
        if production_static:
            import production_cursor_check
            for label,w,h,nonce,profile in PRODUCTION_STATIC_CASES:
                def static_case(label=label,w=w,h=h,nonce=nonce):
                    root=evidence/label
                    execute([harness,root,'cursor-production',w,h,nonce,a.ffmpeg,harness],evidence,label,120,observe=root,production_static=True)
                    require_production_cleanup(root,evidence/(label+'-command.json'),complete=True)
                    result=production_cursor_check.check(root,a.ffmpeg,a.ffprobe)
                    # A static subset never establishes complete family qualification.
                    assert result.get('productionStaticCasesPassed') is True and result.get('productionCursorQualified') is False,'independent static media checker incomplete or rejected'
                before_case=len(failures);attempt(label,static_case)
                if len(failures)!=before_case:break # Unknown cleanup or any failed case forbids the next owned source.
            return
        if cursor_private:
            import cursor_check
            for label,w,h,nonce in [('cursor-borderless-even',320,240,500),('cursor-decorated-even',320,240,501),('cursor-borderless-odd',641,359,502),('cursor-decorated-odd',641,359,503)]:
                def private_case(label=label,w=w,h=h,nonce=nonce):
                    root=evidence/label;execute([harness,root,'cursor-private',w,h,nonce,a.ffmpeg,harness],evidence,label,45,observe=root)
                    require_private_cleanup(root,evidence/(label+'-command.json'),complete=True)
                    cursor_check.check(root/'session-0',a.ffmpeg,a.ffprobe)
                attempt(label,private_case)
                before_cleanup=len(failures)
                attempt(label+'-cleanup',lambda:require_private_cleanup(evidence/label,evidence/(label+'-command.json')))
                if len(failures)!=before_cleanup:break # Unknown/live owners forbid another profile, even after an otherwise successful command.
            return
        if diagnostic:
            attempt('close',lambda:one('close','close',641,359,202,harness,a.ffmpeg));return
        for i,(w,h) in enumerate([(320,240),(641,359),(803,451),(960,540),(161,479)]):attempt(f'geometry-{w}x{h}',lambda w=w,h=h,i=i:one(f'geometry-{w}x{h}','normal',w,h,100+i,harness,a.ffmpeg))
        for i,mode in enumerate(['static','repeat','close','resize','admission-close','admission-resize','marker-failure','lost-worker','transfer-shutdown','stop-shutdown','close-stop']):attempt(mode,lambda mode=mode,i=i:one(mode,mode,641,359,200+i,harness,a.ffmpeg))
        if actual is not None:attempt('actual-executable',lambda:one('actual-executable','normal',803,451,300,actual,a.ffmpeg))
        assert fixture is not None,'fault fixture unavailable; dependent native fault cases not run'
        faultdir=evidence/'fault-tools';faultdir.mkdir()
        fixturetools=evidence.parent/(evidence.name+'-fixture-executables');fixturetools.mkdir()
        for i,mode in enumerate(['blocked','partial','exit7','parent-death']):
            encoder=fixturetools/(mode+'.exe');shutil.copyfile(fixture,encoder);write(encoder.with_suffix('.json'),dict(realFfmpeg=str(a.ffmpeg),mode=mode,evidence=str(faultdir)))
            attempt(mode,lambda mode=mode,encoder=encoder,i=i:one(mode,mode,641,359,400+i,harness,encoder))
        # Executables are provenance inputs, not upload payload. Keep hashes/config/relay receipts only.
        for p in fixturetools.iterdir():p.unlink()
        fixturetools.rmdir()
    attempt('product',product)
    assert source_manifest(repo)==before,'build changed qualified source inputs'
    bound(evidence)
    write(evidence/'artifact-hashes.json',{p.relative_to(evidence).as_posix():sha(p) for p in evidence.rglob('*') if p.is_file()})
    write(evidence/'result.json',dict(scope=scope,privateCursorProof=cursor_private,productionCursorQualified=False,diagnosticOnly=diagnostic or startup_metadata,metadataCollected=(not failures if startup_metadata else None),productionStaticCasesPassed=(not failures if production_static else None),passed=not failures and not diagnostic and not cursor_private and not startup_metadata and not production_static,cursorProofPassed=(not failures if cursor_private else None),diagnosticPassed=(not failures if diagnostic else None),failures=failures,nativeEquality='not a frame equality/admission lane' if startup_metadata else 'inspect each analysis; missing equality is an open coverage gate',geometryTransition='metadata observations only; no geometry is admitted' if startup_metadata else 'inspect resize analysis; old-pool/new-content may remain unobserved',limitations=['Scalar metadata only; no encoder/ARM/epoch/pixels/import or production qualification'] if startup_metadata else ['No actual Tauri/WebView2 Record Stop Edit Play','No microphone/pointer/device/packaging claim','WARP debug-layer failure is unqualified, never skipped']))
    if failures:raise RuntimeError(f'{len(failures)} required gates failed')

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--scope',choices=['full','close-diagnostic','cursor-private','startup-metadata','cursor-production-static'],default='full');p.add_argument('--evidence',type=Path,required=True);p.add_argument('--ffmpeg',type=Path,required=True);p.add_argument('--ffprobe',type=Path,required=True);a=p.parse_args();a.ffmpeg=a.ffmpeg.resolve();a.ffprobe=a.ffprobe.resolve();run(a)
