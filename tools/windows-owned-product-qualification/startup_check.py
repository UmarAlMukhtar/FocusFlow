"""Bounded scalar-only startup diagnostic validation. Never grants recording/import authority."""
import json
from pathlib import Path

SOURCE_NAMES={'source-request.json','source-ready.json','source-observed.json','source-selection.json','source-exit.json','source-cleanup.json','source-actions.json','source-command-1.json','source-ack-1.json','paint-history.json','source.log'}
PROBE_NAMES={'startup-metadata.json','startup-phase.json','metadata-owned.json','metadata-cleanup.json','startup-analysis.json','metadata-handles-open'}

def read(path,limit=65536):
    assert path.is_file() and not path.is_symlink() and path.stat().st_size<=limit,'metadata regular-file/size failure'
    return json.loads(path.read_text())

def scalar_rows(value):
    assert set(value)=={'metadataOnly','selectedHwnd','sourcePid','sourceThread','itemIdentity','sameItemClone','frequency','rowLimit','collectionLimitMs','rows','callbacks','dropped','stopReason','captureClosed','stopComplete','stopError','collectionError','startupError','readErrorCount'},'unexpected metadata authority/state fields'
    assert value['metadataOnly'] is True and value['sameItemClone'] is True and value['stopComplete'] is True
    assert value['startupError'] is None and value['collectionError'] is None and value['stopError'] is None,'metadata native startup/stop failed'
    assert value['rowLimit']==32 and value['collectionLimitMs']==2000 and value['frequency']>0
    assert value['readErrorCount']==0 and value['dropped']==0 and value['captureClosed'] is False,'metadata scalar reads/collection incomplete'
    assert value['stopReason'] in {'rowBudget','collectionDeadline'},'metadata source/capture ended unexpectedly'
    rows=value['rows'];assert 3<=len(rows)<=32
    assert [r['phase'] for r in rows].count('beforeStart')==[r['phase'] for r in rows].count('afterStart')==1
    callbacks=[r for r in rows if r['phase']=='callback'];assert 1<=len(callbacks)<=30 and value['callbacks']==len(callbacks) and len(rows)==len(callbacks)+2
    identity=value['itemIdentity'];assert isinstance(identity,str) and 0<len(identity)<=192
    keys={'phase','q0','q1','itemIdentity','itemSize','contentSize','textureSize','native100ns','readErrors'}
    for row in rows:
        assert set(row)==keys and row['phase'] in {'beforeStart','afterStart','callback'},'unexpected scalar metadata fields'
        assert row['itemIdentity']==identity and row['readErrors']==[] and 0<=row['q0']<=row['q1']
        assert len(row['itemSize'])==2 and all(type(n) is int and 0<n<=16384 for n in row['itemSize'])
        if row['phase']=='callback':
            assert type(row['native100ns']) is int and row['native100ns']>=0
            assert all(len(row[k])==2 and all(type(n) is int and 0<n<=16384 for n in row[k]) for k in ['contentSize','textureSize'])
        else:assert row['contentSize'] is row['textureSize'] is row['native100ns'] is None
    assert all(a['q1']<=b['q0'] for a,b in zip(callbacks,callbacks[1:])),'callback QPC ordering regressed'
    assert (callbacks[-1]['q1']-callbacks[0]['q0'])*1000<=value['frequency']*2000,'metadata collection exceeds2seconds'
    before=next(r for r in rows if r['phase']=='beforeStart');after=next(r for r in rows if r['phase']=='afterStart')
    assert before['q1']<=after['q0'] and all(before['q1']<=r['q0'] for r in callbacks)
    return dict(beforeSize=before['itemSize'],afterSize=after['itemSize'],callbackCount=len(callbacks),callbackSizes=[dict(itemSize=r['itemSize'],contentSize=r['contentSize'],textureSize=r['textureSize'],contentEqualsTexture=r['contentSize']==r['textureSize']) for r in callbacks],itemSizeChanged=any(r['itemSize']!=before['itemSize'] for r in rows),stopReason=value['stopReason'],diagnosticOnly=True,recordingQualified=False)

def check(case,command):
    for p in case.rglob('*'):
        if p.is_file():
            assert not p.is_symlink(),'metadata evidence symlink rejected'
            relative=p.relative_to(case).parts
            assert (len(relative)==1 and p.name in SOURCE_NAMES) or (len(relative)==2 and relative[0]=='startup-probe' and p.name in PROBE_NAMES),'unexpected metadata artifact/media/authority'
    probe=case/'startup-probe';value=read(probe/'startup-metadata.json');analysis=scalar_rows(value)
    assert all(type(value[k]) is int and value[k]>0 for k in ['selectedHwnd','sourcePid','sourceThread'])
    phase=read(probe/'startup-phase.json',4096);assert phase['phase']=='finished' and phase['qpc']>=max(r['q1'] for r in value['rows'])
    native_owned=read(probe/'metadata-owned.json',4096);assert set(native_owned)=={'parent','worker','qpc'}
    selection=read(case/'source-selection.json',1024);assert selection['sourcePid']==value['sourcePid'] and selection['hwnd']==value['selectedHwnd'] and selection['driverPid']==native_owned['parent']
    cleanup=read(probe/'metadata-cleanup.json',16384);assert cleanup['workerExit']==0 and cleanup['workerSignaled'] is True and cleanup['fallback'] is False and cleanup['error'] is None
    receipt=read(command,16384);assert receipt['exitCode']==0 and receipt['error'] is None and len(receipt['ownership'])==1
    owned=receipt['ownership'][0];assert 'encoder' not in owned and owned['worker']==cleanup['worker'] and owned['signaled']==[True] and owned['fallback'] is False and all(owned[k]==native_owned[k] for k in native_owned)
    source=read(case/'source-cleanup.json',4096);assert source['pid']==value['sourcePid'] and source['observed'] is True and source['exitCode']==0 and source['signaled'] is True and source['fallback'] is False
    actions=read(case/'source-actions.json',4096);assert actions['overflow'] is False and len(actions['records'])==1 and actions['records'][0]['action']=='close' and actions['records'][0]['win32Succeeded'] is True
    analysis.update(independentWorkerSignaled=True,sourceClosed=True,noEncoder=True,noPixels=True,productionCursorQualified=False)
    (probe/'startup-analysis.json').write_text(json.dumps(analysis,indent=2)+'\n');return analysis
