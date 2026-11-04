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

def require_test_count(text,count):
    assert re.search(r'test result: ok\. '+str(count)+r' passed; 0 failed; 0 ignored;',text), 'required native tests did not execute without skips'

def require_exit7(case):
    trace=json.loads((case/'trace.json').read_text(encoding='utf-8'))
    prefix=json.loads((case/'consumer-prefix.json').read_text(encoding='utf-8'))
    assert trace['encoderExitCode']==7 and trace['nonEncoderFailure'] is False and trace['failure']=='Encoder exited 7', 'exit7 was not the sole encoder exit failure'
    assert trace['supervisorTerminated'] is None and trace['overflow'] is False, 'exit7 had Supervisor or observation failure'
    assert trace['selected']==trace['written']>0 and len(trace['writes'])==trace['written'], 'exit7 did not drain every selected frame'
    frame_bytes=(trace['width']+trace['width']%2)*(trace['height']+trace['height']%2)*4
    assert all(w['ok'] is True and w['bytes']==frame_bytes for w in trace['writes']), 'exit7 had an incomplete writer result'
    assert prefix['eof'] is True and prefix['notWriterProgress'] is True and prefix['consumerBytes']==trace['written']*frame_bytes, 'exit7 consumer did not reach EOF after all frames'

def bound(root):
    size=0
    for p in root.rglob('*'):
        is_link=False
        try:
            is_link=p.is_symlink()
            if not p.is_file():continue
            n=p.stat().st_size
        except FileNotFoundError:
            # The owned initial handshake atomically renames this one transient file.
            relative=p.relative_to(root).parts
            if len(relative)==3 and relative[0] in {'cursor-borderless-even','cursor-decorated-even','cursor-borderless-odd','cursor-decorated-odd'} and relative[1:]==('session-0','source-request.pending') and not is_link:continue
            if not is_link and relative[:2]==('startup-metadata','session-0'):
                if len(relative)==3 and relative[2] in {f'{n}.pending' for n in ['source-request','source-ready','source-observed','source-selection','source-exit','source-cleanup','source-actions','source-command-1','source-ack-1']}:continue
                if len(relative)==4 and relative[2]=='startup-probe' and relative[3] in {'startup-phase.pending','startup-metadata.pending','metadata-owned.pending','metadata-cleanup.pending'}:continue
            raise
        size+=n
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

class GeneratedSources:
    def __init__(self,harness,root,env):self.harness=harness;self.root=root;self.env=env;self.children={}
    def poll(self,driver_pid):
        for request in self.root.glob('session-*/source-request.json'):
            case=request.parent
            if case in self.children:continue
            assert not case.is_symlink() and case.name in {'session-0','session-1','session-2'} and len(self.children)<3,'unexpected source session'
            data=fixture_read(request)
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
            self.children[case]=dict(proc=proc,argv=argv,observed=False,aliveBeforeParentKillQpc=None,aliveAfterProductExitQpc=None)
        for case,item in self.children.items():
            proc=item['proc'];code=proc.poll()
            if code is not None:
                if not (case/'source-exit.json').exists():fixture_write(case/'source-exit.json',dict(pid=proc.pid,exitCode=code))
                assert code==0 and any(fixture_read(p).get('action')=='close' for p in case.glob('source-command-*.json')),'source exited unexpectedly'
                continue
            ready=case/'source-ready.json'
            if not item['observed'] and ready.is_file():
                data=fixture_read(ready)
                assert set(data)=={'pid','hwnd'} and type(data['pid']) is int and type(data['hwnd']) is int
                assert data['pid']==proc.pid!=driver_pid and data['hwnd']>0,'wrong source process identity'
                assert window_pid(data['hwnd'])==proc.pid,'HWND does not belong to exact source process'
                fixture_write(case/'source-observed.json',data);item['observed']=True
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

def execute(argv,root,name,timeout=600,env=None,observe=None,parent_death=False):
    if name=='startup-metadata':require_metadata_disposable_context()
    started=time.monotonic();opened={};media={};killed_at=None;kill_qpc=None;receipts=[];error=None;error_filename=None;error_traceback=None
    log=root/(name+'.log');receipt=root/(name+'-command.json')
    sources=GeneratedSources(argv[0],observe,env) if observe else None
    with log.open('wb') as out:
        proc=subprocess.Popen([str(x) for x in argv],stdout=out,stderr=subprocess.STDOUT,env=env)
        try:
            while proc.poll() is None:
                bound(root)
                assert time.monotonic()-started<timeout,'outer watchdog fired; never product success'
                if observe:
                    sources.poll(proc.pid)
                    ownership_paths=list(observe.glob('session-*/owned.json'))
                    ownership_paths+=list(observe.glob('session-*/without/owned.json'))+list(observe.glob('session-*/with/owned.json'))+list(observe.glob('session-*/alpha-without/owned.json'))+list(observe.glob('session-*/alpha-with/owned.json'))+list(observe.glob('session-*/challenge/owned.json'))+list(observe.glob('session-*/closed/owned.json'))
                    ownership_paths+=list(observe.glob('session-0/startup-probe/metadata-owned.json'))
                    for path in ownership_paths:
                        if path in opened:continue
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
            for _,handles in opened.values():
                for h in handles:
                    if not h.signaled():h.kill()
                    h.close()
            write(receipt,dict(argv=[str(x) for x in argv],exitCode=proc.returncode,seconds=time.monotonic()-started,error=error,errorFilename=error_filename,errorTraceback=error_traceback,logSha256=sha(log),ownership=receipts))
    if error:raise RuntimeError(error)
    return log

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
    scope=getattr(a,'scope','full');assert scope in {'full','close-diagnostic','cursor-private','startup-metadata'},'unknown qualification scope'
    cursor_private=scope=='cursor-private';startup_metadata=scope=='startup-metadata'
    disposable=require_metadata_disposable_context() if startup_metadata else require_cursor_disposable_context() if cursor_private else None
    diagnostic=scope=='close-diagnostic'
    expected_branch='codex-windows-startup-metadata' if startup_metadata else 'codex-windows-cursor-qualification' if cursor_private else 'codex-windows-owned-close-diagnostic' if diagnostic else 'codex-windows-owned-product-qualification'
    observed_ref=os.getenv('GITHUB_REF')
    assert observed_ref is None or observed_ref=='refs/heads/'+expected_branch,'qualification scope/ref mismatch'
    if diagnostic:print('DIAGNOSTIC SUBSET: close only; cannot establish full qualification PASS')
    repo=Path(__file__).resolve().parents[2];os.chdir(repo)
    if cursor_private or startup_metadata:assert subprocess.check_output(['git','rev-parse','HEAD'],text=True,encoding='ascii').strip()==disposable['sha'],'checkout/run SHA mismatch before native tools'
    evidence=a.evidence.resolve();evidence.mkdir();assert sha(a.ffmpeg)==FFMPEG_SHA and sha(a.ffprobe)==FFPROBE_SHA
    before=source_manifest(repo);write(evidence/'source-manifest.json',before)
    write(evidence/'provenance.json',dict(scope=scope,privateCursorProof=cursor_private,disposableContext=disposable,diagnosticOnly=diagnostic or startup_metadata,metadataOnly=startup_metadata,expectedBranch=expected_branch,observedRef=observed_ref,commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True,encoding='ascii').strip(),runnerImage=os.getenv('ImageVersion'),runnerOS=os.getenv('RUNNER_OS'),ffmpegSha256=sha(a.ffmpeg),ffprobeSha256=sha(a.ffprobe),archiveSha256=ARCHIVE_SHA,python=sys.version,platform=sys.platform,toolchain='1.89.0',profile='scalar metadata only; no encoder/ARM/admission' if startup_metadata else 'debug; H264 libx264 veryfast CRF23 yuv420p B0 30fps 30000-timescale',environment={k:os.getenv(k) for k in ['CARGO_TARGET_DIR','VCToolsVersion','WindowsSDKVersion','PROCESSOR_IDENTIFIER']}))
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
    def require_tests(argv,name,count):
        log=execute(argv,evidence,name)
        require_test_count(log.read_text(encoding='utf-8',errors='replace'),count)
    if not diagnostic and not cursor_private and not startup_metadata:
        attempt('core-publication',lambda:require_tests(['cargo','+1.89.0','test','--locked','--manifest-path',core,'--lib','capture::windows::tests::','--','--test-threads=1','--nocapture'],'core-publication',10))
        attempt('core-import',lambda:require_tests(['cargo','+1.89.0','test','--locked','--manifest-path',core,'--test','windows_capture','--','--test-threads=1','--nocapture','--skip','common::child_writer'],'core-import',WINDOWS_IMPORT_TEST_COUNT))
        attempt('mapping-debug',lambda:require_tests(['cargo','+1.89.0','test','--locked','--manifest-path',mapping,'--lib','frame::mapping_tests::','--','--test-threads=1','--nocapture'],'mapping-debug',4))
    def pipes():
        exe=build_exe(repo,evidence,core,'cfr_grid_windows',True);env=os.environ.copy();env['CORE_QUAL_EVIDENCE']=str(evidence/'core-pipes')
        execute([exe],evidence,'core-pipes',90,env)
    if not diagnostic and not cursor_private and not startup_metadata:attempt('core-pipes',pipes)
    if cursor_private:attempt('cursor-renderer',lambda:require_tests(['cargo','+1.89.0','test','--locked','--manifest-path',core,'--test','cursor_render','lossless_rgba_compositing_isolates_alpha_and_hotspot','--','--test-threads=1'],'cursor-renderer',1))
    if cursor_private:attempt('cursor-portable',lambda:require_tests(['cargo','+1.89.0','test','--locked','--manifest-path',core,'--test','windows_cursor_private','--','--test-threads=1','--nocapture','--skip','common::child_writer'],'cursor-portable',6))
    def product():
        built={}
        targets=[('windows_owned_qualification',True)] if diagnostic or cursor_private or startup_metadata else [('FocusFlow',False),('windows_owned_qualification',True),('windows_encoder_fixture',True)]
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
        if not diagnostic and not cursor_private and not startup_metadata:attempt('native-platform',lambda:execute([harness,'--platform',evidence/'native-platform.json'],evidence,'native-platform',20))
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
        if cursor_private:
            import cursor_check
            for label,w,h,nonce in [('cursor-borderless-even',320,240,500),('cursor-decorated-even',320,240,501),('cursor-borderless-odd',641,359,502),('cursor-decorated-odd',641,359,503)]:
                def private_case(label=label,w=w,h=h,nonce=nonce):
                    root=evidence/label;execute([harness,root,'cursor-private',w,h,nonce,a.ffmpeg,harness],evidence,label,45,observe=root)
                    cursor_check.check(root/'session-0',a.ffmpeg,a.ffprobe)
                attempt(label,private_case)
                if failures:break # Required first profile failure is FAIL, never an unavailable/skip.
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
    write(evidence/'result.json',dict(scope=scope,privateCursorProof=cursor_private,productionCursorQualified=False,diagnosticOnly=diagnostic or startup_metadata,metadataCollected=(not failures if startup_metadata else None),passed=not failures and not diagnostic and not cursor_private and not startup_metadata,cursorProofPassed=(not failures if cursor_private else None),diagnosticPassed=(not failures if diagnostic else None),failures=failures,nativeEquality='not a frame equality/admission lane' if startup_metadata else 'inspect each analysis; missing equality is an open coverage gate',geometryTransition='metadata observations only; no geometry is admitted' if startup_metadata else 'inspect resize analysis; old-pool/new-content may remain unobserved',limitations=['Scalar metadata only; no encoder/ARM/epoch/pixels/import or production qualification'] if startup_metadata else ['No actual Tauri/WebView2 Record Stop Edit Play','No microphone/pointer/device/packaging claim','WARP debug-layer failure is unqualified, never skipped']))
    if failures:raise RuntimeError(f'{len(failures)} required gates failed')

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--scope',choices=['full','close-diagnostic','cursor-private','startup-metadata'],default='full');p.add_argument('--evidence',type=Path,required=True);p.add_argument('--ffmpeg',type=Path,required=True);p.add_argument('--ffprobe',type=Path,required=True);a=p.parse_args();a.ffmpeg=a.ffmpeg.resolve();a.ffprobe=a.ffprobe.resolve();run(a)
