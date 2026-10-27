"""Finite generated capture and transport faults; no product authority is emitted."""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import time
from analyze import analyze

FFMPEG_SHA256='5af82a0d4fe2b9eae211b967332ea97edfc51c6b328ca35b827e73eac560dc0d'


def rows(path):
    if not path.exists():return []
    if path.stat().st_size>2*1024*1024:raise ValueError('receipt exceeds 2MiB')
    # An in-progress final line is not a complete receipt yet.
    return [json.loads(line) for line in path.read_text().splitlines(keepends=True) if line.endswith('\n')]


def qualify_fault(scenario,events,ledger,cleanup,parent_killed,proof=None):
    if not cleanup:return False
    if scenario=='parent-death':
        if not parent_killed or not proof or proof.get('fallbackUsed') or proof.get('childrenAliveBeforeKill') is not True:return False
        parked=[r for r in events if r['event']=='jobProofParked']
        starts=[r for r in events if r['event']=='startup']
        encoders=[r for r in events if r['event']=='encoderOwned']
        if not len(parked)==len(starts)==len(encoders)==1:return False
        p,start,encoder=parked[0],starts[0],encoders[0];f=start['frequency']
        return (f>0 and p.get('workerPid')==start['pid'] and p.get('encoderPid')==encoder['pid']
                and encoder.get('inJob') is True and encoder.get('transportFault') is False
                and p.get('completeWrites',0)>=1 and p.get('writerIdle') is True and p.get('parkSeconds')==10
                and p['qpc']<=proof['killQpc']<=p['qpc']+f
                and proof['killQpc']<=proof['observedQpc']<=proof['killQpc']+3*f
                and proof['observedQpc']<p['qpc']+10*f)
    terminal=[r for r in events if r['event']=='terminal']
    if len(terminal)!=1:return False
    t=terminal[0]
    if not (t.get('reaped') is True and t.get('writerDone') is True and t.get('completeWrites')==0):return False
    writes=[r for r in ledger if r['event']=='written']
    if len(writes)!=1 or not 0<=writes[0]['bytesWritten']<640*360*4:return False
    failure=t.get('failure') or ''
    if scenario=='blocked':
        return t.get('supervisorFallback') is False and failure=='writer stalled over 1 second' and any(r['event']=='supervisorTermination' and r.get('requestSucceeded') is True for r in events)
    return scenario=='exit-midframe' and failure.startswith('writer failed:') and writes[0]['bytesWritten']>0


class ProcessObservations:
    """Hold synchronization handles before termination so PID reuse is not exit proof."""
    def __init__(self):
        self.kernel=ctypes.WinDLL('kernel32',use_last_error=True)
        self.kernel.OpenProcess.argtypes=[wintypes.DWORD,wintypes.BOOL,wintypes.DWORD]
        self.kernel.OpenProcess.restype=wintypes.HANDLE
        self.kernel.WaitForSingleObject.argtypes=[wintypes.HANDLE,wintypes.DWORD]
        self.kernel.WaitForSingleObject.restype=wintypes.DWORD
        self.kernel.CloseHandle.argtypes=[wintypes.HANDLE]
        self.kernel.CloseHandle.restype=wintypes.BOOL
        self.kernel.TerminateProcess.argtypes=[wintypes.HANDLE,wintypes.UINT]
        self.kernel.TerminateProcess.restype=wintypes.BOOL
        self.kernel.QueryPerformanceCounter.argtypes=[ctypes.POINTER(ctypes.c_longlong)]
        self.kernel.QueryPerformanceCounter.restype=wintypes.BOOL
        self.handles={}

    def qpc(self):
        ticks=ctypes.c_longlong()
        if not self.kernel.QueryPerformanceCounter(ctypes.byref(ticks)):raise OSError('QueryPerformanceCounter failed')
        return ticks.value

    def terminate_remaining(self):
        requests={}
        for name,(pid,handle) in self.handles.items():
            if self.kernel.WaitForSingleObject(handle,0)!=0:
                requests[name]=dict(pid=pid,requestSucceeded=bool(self.kernel.TerminateProcess(handle,5)))
        return requests

    def capture(self,events):
        for row in events:
            if row['event'] in ('workerOwned','encoderOwned'):
                key=row['event']
                if key not in self.handles:
                    handle=self.kernel.OpenProcess(0x00100001,False,row['pid']) # SYNCHRONIZE + TERMINATE, exact retained child identity.
                    if handle:self.handles[key]=(row['pid'],handle)

    def observe(self):
        result={}
        for name,(pid,handle) in self.handles.items():
            wait=self.kernel.WaitForSingleObject(handle,0)
            result[name]=dict(pid=pid,handleObservedBeforeExit=True,terminated=wait==0,running=wait==258)
        return result,len(result)==2 and all(r['terminated'] for r in result.values())

    def close(self):
        for _,handle in self.handles.values():self.kernel.CloseHandle(handle)


def run(exe,evidence,ffprobe,ffmpeg):
    if os.name!='nt':raise RuntimeError('native Windows evidence requires Windows')
    exe,ffmpeg,ffprobe=map(lambda p:str(Path(p).resolve(strict=True)),(exe,ffmpeg,ffprobe))
    if hashlib.sha256(Path(ffmpeg).read_bytes()).hexdigest()!=FFMPEG_SHA256:
        raise RuntimeError('FFmpeg differs from pinned verified archive')
    evidence=Path(evidence).resolve();evidence.mkdir(parents=True,exist_ok=False)
    summary=dict(nativeWindowsExecution=True,authorityAccepted=False,platform=platform.platform(),
                 python=platform.python_version(),encoderSha256=FFMPEG_SHA256,scenarios=[])
    for scenario in ('first','static','animated','stall','close','blocked','exit-midframe','parent-death'):
        case=evidence/scenario;fault=scenario in ('blocked','exit-midframe','parent-death')
        item=dict(scenario=scenario,status='native-unavailable-or-failed',captureQualified=False)
        parent_log=evidence/(scenario+'-parent.jsonl');observations=ProcessObservations();parent_killed=False;proof=None
        process=None;cleanup_deadline=None
        try:
            with parent_log.open('wb') as stdout,(evidence/(scenario+'-parent.stderr')).open('wb') as stderr:
                process=subprocess.Popen([exe,'run',scenario,str(case),ffmpeg],stdout=stdout,stderr=stderr)
                deadline=time.monotonic()+28
                while process.poll() is None and time.monotonic()<deadline:
                    events=rows(case/'native.jsonl');ledger=rows(case/'selection.jsonl')
                    observations.capture(rows(parent_log)+events)
                    armed=[r for r in events if r['event']=='jobProofParked']
                    if scenario=='parent-death' and len(observations.handles)==2 and len(armed)==1:
                        before,_=observations.observe();kill_qpc=observations.qpc()
                        start=next(r for r in events if r['event']=='startup')
                        if not all(r['running'] for r in before.values()) or not 0<=kill_qpc-armed[0]['qpc']<=start['frequency']:
                            raise ValueError('Job proof was not armed with live children and sufficient park time')
                        proof=dict(killQpc=kill_qpc,childrenAliveBeforeKill=True,fallbackUsed=False)
                        process.kill();parent_killed=True;item['parentOnlyTerminationRequested']=True;break
                    time.sleep(.02)
                if process.poll() is None and not parent_killed:
                    process.kill();item['deadlineExceeded']=True
                cleanup_deadline=time.monotonic()+3
                try:item['nativeExitCode']=process.wait(timeout=max(.001,cleanup_deadline-time.monotonic()))
                except subprocess.TimeoutExpired:item['terminationUnconfirmed']=True
            observations.capture(rows(parent_log)+rows(case/'native.jsonl'))
            # Leave one second of the shared three-second cleanup budget for exact-handle fallback.
            deadline=cleanup_deadline-1 if parent_killed else cleanup_deadline
            while True:
                observed,cleanup=observations.observe()
                if cleanup or time.monotonic()>=deadline:break
                time.sleep(.02)
            if proof is not None:proof['observedQpc']=observations.qpc()
            item.update(ownedProcessObservation=observed,ownedCleanupConfirmed=cleanup)
            if not cleanup:
                requests=observations.terminate_remaining();item['fallbackTermination']=requests
                if proof is not None:proof['fallbackUsed']=True
                while time.monotonic()<cleanup_deadline:
                    after,fallback_cleanup=observations.observe()
                    if fallback_cleanup:break
                    time.sleep(.02)
                after,fallback_cleanup=observations.observe()
                item.update(postFallbackOwnedObservation=after,fallbackCleanupConfirmed=fallback_cleanup)
            if proof is not None:item['jobProof']=proof
            events=rows(case/'native.jsonl');ledger=rows(case/'selection.jsonl')
            if (case/'screen.mp4').exists():
                # Keep full saved-media diagnostics even when native generation rejected.
                result=analyze(case,scenario,ffprobe,ffmpeg);item['analysisStatus']=result['status']
            else:result={}
            if fault:
                item['status']='expected-failure-cleanup-verified' if qualify_fault(scenario,events,ledger,cleanup,parent_killed,proof) else 'fault-not-proved'
            elif item.get('nativeExitCode')==0 and cleanup and result.get('status')=='owned-feed-verified':
                if scenario=='close':
                    destroyed=[r for r in rows(parent_log) if r['event']=='destroyAfterAck']
                    stops=[r for r in events if r['event']=='stopInstalled'];closed=[r for r in events if r['event']=='sourceClosed']
                    if len(destroyed)!=1 or not stops[0]['qpc']<=destroyed[0]['qpc']<=closed[0]['qpc']:
                        raise ValueError('close did not destroy source after installed cutoff and before sourceClosed')
                item.update(status='owned-feed-verified',captureQualified=True)
        except (OSError,ValueError,KeyError,TypeError,subprocess.SubprocessError) as error:
            item['error']=str(error)
        finally:
            if cleanup_deadline is None:cleanup_deadline=time.monotonic()+3
            if process is not None and process.poll() is None:
                process.kill()
                try:process.wait(timeout=max(.001,cleanup_deadline-time.monotonic()))
                except subprocess.TimeoutExpired:item['terminationUnconfirmed']=True
            observed,exited=observations.observe()
            if not exited and observations.handles:
                item.setdefault('fallbackTermination',observations.terminate_remaining())
                if proof is not None:proof['fallbackUsed']=True
                fallback_deadline=cleanup_deadline
                while time.monotonic()<fallback_deadline:
                    observed,exited=observations.observe()
                    if exited:break
                    time.sleep(.02)
                item.update(finalOwnedObservation=observed,finalCleanupConfirmed=exited)
            observations.close()
        summary['scenarios'].append(item)
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    hashes={str(p.relative_to(evidence)):hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(evidence.rglob('*')) if p.is_file()}
    (evidence/'evidence-sha256.json').write_text(json.dumps(hashes,indent=2)+'\n')
    print(json.dumps(summary,indent=2))
    return 0 if all(s['status'] in ('owned-feed-verified','expected-failure-cleanup-verified') for s in summary['scenarios']) else 2


if __name__=='__main__':
    parser=argparse.ArgumentParser()
    for name in ('exe','evidence','ffprobe','ffmpeg'):parser.add_argument('--'+name,required=True)
    args=parser.parse_args();raise SystemExit(run(args.exe,args.evidence,args.ffprobe,args.ffmpeg))
