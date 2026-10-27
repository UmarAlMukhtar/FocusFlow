import copy
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from analyze import analyze, decode, summarize


def fixture():
    events = [dict(event='startup', frequency=3_000_000, cursorRequest='WithoutCursor'),
              dict(event='admission', qpc=3_000_000), dict(event='captureStarted'),
              dict(event='encoderOwned', pid=42, inJob=True)]
    ledger = []
    for seq, t in enumerate((10_000_000, 10_200_000, 10_400_000)):
        q = 3_000_000 + seq*60_000
        events.append(dict(event='snapshot', seq=seq, native100ns=t, width=640, height=360,
                           id=seq, cornersValid=True, callbackQpc=q+1, copyStartQpc=q+2,
                           copyEndQpc=q+3, copyCompleteQpc=q+4, preRoll=False))
        decision = 3_100_000+seq*100_000
        ledger.append(dict(event='selected', ordinal=seq, sourceSeq=seq, sourceNative100ns=t,
                           sourceId=seq, decisionQpc=decision, targetNumerator=300_000_000+seq*10_000_000,
                           targetDenominator=300_000_000,deliveredThroughSeq=min(2,seq+1)))
        ledger.append(dict(event='written', ordinal=seq, writeBeginQpc=decision+1,
                           writeEndQpc=decision+10, bytesWritten=640*360*4))
    events += [dict(event='stopInstalled', qpc=3_300_000, requestQpc=3_299_999, reason='stop'),
               dict(event='terminal', epoch100ns=10_000_000, stopQpc=3_300_000, expectedCells=3,
                    completeWrites=3, ffmpegExit=0, reaped=True, writerDone=True,
                    sourceQueueHigh=1, futureQueueHigh=2, outputQueueHigh=1, ownedHigh=4,
                    failure=None)]
    probe = dict(streams=[dict(index=0, codec_type='video', width=640, height=360,time_base='1/30000')],
                 frames=[dict(media_type='video',stream_index=0,pts=n*1000,duration=1000) for n in range(3)],
                 packets=[dict(stream_index=0,pts=n*1000,duration=1000) for n in (2,0,1)])
    pixels=[dict(id=n,sourceTag=n,cornersValid=True) for n in range(3)]
    return events,probe,pixels,ledger


def image(ident,tag):
    rgb=bytearray([96]*(640*360*3))
    for x,y,color in [(0,0,(255,0,0)),(608,0,(0,255,0)),(0,328,(0,0,255)),(608,328,(255,255,0))]:
        for row in range(y,y+32):
            for col in range(x,x+32): rgb[(row*640+col)*3:(row*640+col)*3+3]=bytes(color)
    for y,h,value in [(128,64,ident),(224,32,tag)]:
        for b in range(16):
            c=240 if value & (1<<b) else 12
            for row in range(y,y+h):
                for col in range(32+b*32,56+b*32):rgb[(row*640+col)*3:(row*640+col)*3+3]=bytes([c]*3)
    return rgb


class AnalysisTests(unittest.TestCase):
    def check(self, data=None, scenario='animated'):
        e,p,x,l=data or fixture()
        return summarize(e,p,x,scenario,l)

    def test_exact_all_saved_mapping_non_10mhz_qpc(self):
        r=self.check();self.assertEqual(r['status'],'owned-feed-verified')
        self.assertEqual(r['savedEndpointSeconds'],'1/10');self.assertFalse(r['authorityAccepted'])

    def test_tag_and_original_id_are_independent(self):
        p=decode(image(165,37));self.assertEqual((p['id'],p['sourceTag']),(165,37));self.assertTrue(p['cornersValid'])
        self.assertRaises(ValueError,decode,b'bad')

    def test_each_corruption_rejects(self):
        for defect in ('tag','id','corner','omitted','blacktail','pts','duration','endpoint','future',
                       'notmaximal','incomplete','exit','preroll','duplicate','geometry','unknownstream',
                       'audio','packet','earlydecision','copybracket','queue','origin','cutoff','ordinal'):
            e,p,x,l=fixture()
            if defect=='tag':x[1]['sourceTag']=0
            if defect=='id':x[1]['id']=77
            if defect=='corner':x[1]['cornersValid']=False
            if defect=='omitted':l.pop(2)
            if defect=='blacktail':x.append(dict(id=0,sourceTag=0,cornersValid=False))
            if defect=='pts':p['frames'][0]['pts']=1
            if defect=='duration':del p['frames'][1]['duration']
            if defect=='endpoint':p['frames'][-1]['duration']=999;p['packets'][0]['duration']=999
            if defect=='future':l[0].update(sourceSeq=1,sourceNative100ns=10_200_000,sourceId=1);x[0].update(id=1,sourceTag=1)
            if defect=='notmaximal':l[2].update(sourceSeq=0,sourceNative100ns=10_000_000,sourceId=0);x[1].update(id=0,sourceTag=0)
            if defect=='incomplete':l[3]['bytesWritten']=123
            if defect=='exit':e[-1]['ffmpegExit']=1
            if defect=='preroll':e[4]['preRoll']=True
            if defect=='duplicate':e[5]['native100ns']=e[4]['native100ns']
            if defect=='geometry':e[5]['width']=1
            if defect=='unknownstream':p['packets'][0]['stream_index']=7
            if defect=='audio':p['streams'].append(dict(index=1,codec_type='audio'))
            if defect=='packet':p['packets'][0]['pts']=5
            if defect=='earlydecision':l[0]['decisionQpc']=3_099_999
            if defect=='copybracket':e[5]['copyEndQpc']=1
            if defect=='queue':e[-1]['ownedHigh']=23
            if defect=='origin':e[-1]['epoch100ns']+=1
            if defect=='cutoff':e[-1]['stopQpc']-=1
            if defect=='ordinal':l[2]['ordinal']=2
            with self.subTest(defect=defect),self.assertRaises(ValueError):self.check((e,p,x,l))

    def test_idle_holds_and_late_arrival_are_truthful_not_latency_failures(self):
        e,p,x,l=fixture()
        # Source 1 is delivered after decision 1, despite its old native time.
        e[5].update(callbackQpc=3_210_000,copyStartQpc=3_210_001,copyEndQpc=3_210_002,copyCompleteQpc=3_210_003)
        # Keep timestamps/delivery ordered, as native callback delivery is serial.
        e[6].update(callbackQpc=3_220_000,copyStartQpc=3_220_001,copyEndQpc=3_220_002,copyCompleteQpc=3_220_003)
        l[0]['deliveredThroughSeq']=0
        l[2].update(sourceSeq=0,sourceNative100ns=10_000_000,sourceId=0,deliveredThroughSeq=0);x[1].update(id=0,sourceTag=0)
        r=self.check((e,p,x,l));self.assertEqual(r['status'],'owned-feed-verified');self.assertGreater(r['lateObservations'],0)
        e,p,x,l=fixture();e[4:7]=[e[4]]
        for row in l:
            if row['event']=='selected':row.update(sourceSeq=0,sourceNative100ns=10_000_000,sourceId=0,deliveredThroughSeq=0)
        for row in x:row.update(id=0,sourceTag=0)
        self.assertEqual(self.check((e,p,x,l),'static')['status'],'owned-feed-verified')

    def test_equal_qpc_ticks_use_recorded_delivery_frontier(self):
        e,p,x,l=fixture()
        e[5].update(callbackQpc=3_199_997,copyStartQpc=3_199_998,copyEndQpc=3_199_999,copyCompleteQpc=3_200_000)
        e[6].update(callbackQpc=3_220_000,copyStartQpc=3_220_001,copyEndQpc=3_220_002,copyCompleteQpc=3_220_003)
        l[0]['deliveredThroughSeq']=0
        l[2].update(sourceSeq=0,sourceNative100ns=10_000_000,sourceId=0,deliveredThroughSeq=0)
        x[1].update(id=0,sourceTag=0)
        self.assertGreater(self.check((e,p,x,l))['lateObservations'],0)

    def test_close_and_stall_require_observation(self):
        for case in ('close','stall'):
            with self.subTest(case=case),self.assertRaises(ValueError):self.check(scenario=case)


class RealMediaTests(unittest.TestCase):
    @unittest.skipUnless(os.environ.get('PROBE_FFMPEG') and os.environ.get('PROBE_FFPROBE'),'exact media tool paths required')
    def test_same_rawvideo_command_all_saved_frames_and_failure_receipts(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as t:
            root=Path(t);raw=bytearray()
            for n in range(3):
                rgb=image(n,n)
                for i in range(0,len(rgb),3):raw.extend((rgb[i+2],rgb[i+1],rgb[i],255))
            command=[os.environ['PROBE_FFMPEG'],'-hide_banner','-loglevel','warning','-nostdin','-n',
                     '-f','rawvideo','-pixel_format','bgra','-video_size','640x360','-framerate','30','-i','pipe:0',
                     '-map','0:v:0','-an','-c:v','libx264','-preset','ultrafast','-crf','0','-pix_fmt','yuv444p',
                     '-bf','0','-fps_mode','passthrough','-video_track_timescale','30000',str(root/'screen.mp4')]
            subprocess.run(command,input=raw,check=True,timeout=30)
            e,_,_,l=fixture()
            def save():
                (root/'native.jsonl').write_text(''.join(json.dumps(r)+'\n' for r in e))
                (root/'selection.jsonl').write_text(''.join(json.dumps(r)+'\n' for r in l))
            save();r=analyze(root,'animated',os.environ['PROBE_FFPROBE'],os.environ['PROBE_FFMPEG'])
            self.assertEqual(r['status'],'owned-feed-verified',r);self.assertEqual(r['savedEndpointSeconds'],'1/10')
            self.assertEqual(r['savedTags'],[0,1,2]);self.assertFalse(r['authorityAccepted'])
            e[5]['id']=None;save();r=analyze(root,'animated',os.environ['PROBE_FFPROBE'],os.environ['PROBE_FFMPEG'])
            self.assertEqual(r['status'],'analysis-failed');self.assertEqual(r['savedTags'],[0,1,2]);self.assertIn('native marker',r['error'])
            self.assertEqual(json.loads((root/'analysis.json').read_text()),r)
            self.assertFalse((root/'decode-temporary.rgb').exists())
            (root/'screen.mp4').write_bytes(b'invalid generated media')
            r=analyze(root,'first',os.environ['PROBE_FFPROBE'],os.environ['PROBE_FFMPEG'])
            self.assertEqual(r['status'],'analysis-failed');self.assertTrue((root/'analysis-commands.json').exists())


class FaultReceiptTests(unittest.TestCase):
    def test_expected_fault_requires_named_failure_partial_write_and_observed_cleanup(self):
        from run_probe import qualify_fault
        events=[dict(event='terminal',failure='writer failed: broken pipe',reaped=True,writerDone=True,completeWrites=0)]
        ledger=[dict(event='written',bytesWritten=4096)]
        self.assertTrue(qualify_fault('exit-midframe',events,ledger,True,False))
        for cleanup, rows in [(False,ledger),(True,[]),(True,[dict(event='written',bytesWritten=921600)])]:
            self.assertFalse(qualify_fault('exit-midframe',events,rows,cleanup,False))
        events[0]['failure']='unrelated failure'
        self.assertFalse(qualify_fault('exit-midframe',events,ledger,True,False))
        self.assertFalse(qualify_fault('parent-death',[],[],True,False))
        self.assertFalse(qualify_fault('parent-death',[],[],True,True))

    def test_fallback_cleanup_cannot_masquerade_as_supervisor_proof(self):
        from run_probe import qualify_fault
        events=[dict(event='terminal',failure='writer stalled over 1 second',reaped=True,writerDone=True,
                     completeWrites=0,supervisorFallback=True),
                dict(event='supervisorTermination',requestSucceeded=True)]
        ledger=[dict(event='written',bytesWritten=65536)]
        self.assertFalse(qualify_fault('blocked',events,ledger,True,False))
        events[0]['supervisorFallback']=False
        self.assertTrue(qualify_fault('blocked',events,ledger,True,False))

class JobProofTests(unittest.TestCase):
    def test_job_proof_requires_armed_identity_and_exit_before_park_escape(self):
        from run_probe import qualify_fault
        events=[dict(event='startup',frequency=1000,pid=40),
                dict(event='encoderOwned',pid=42,inJob=True,transportFault=False),
                dict(event='jobProofParked',qpc=1000,parkSeconds=10,workerPid=40,encoderPid=42,
                     completeWrites=1,writerIdle=True)]
        proof=dict(killQpc=1100,observedQpc=1300,fallbackUsed=False,childrenAliveBeforeKill=True)
        self.assertTrue(qualify_fault('parent-death',events,[],True,True,proof))
        for defect in ('missing-arm','identity','no-write','not-idle','late-kill','late-observation','fallback','already-dead','not-killed','not-exited'):
            changed=copy.deepcopy(events);timing=dict(proof);cleanup=True;killed=True
            if defect=='missing-arm':changed.pop()
            if defect=='identity':changed[-1]['encoderPid']=99
            if defect=='no-write':changed[-1]['completeWrites']=0
            if defect=='not-idle':changed[-1]['writerIdle']=False
            if defect=='late-kill':timing['killQpc']=2500
            if defect=='late-observation':timing['observedQpc']=11000
            if defect=='fallback':timing['fallbackUsed']=True
            if defect=='already-dead':timing['childrenAliveBeforeKill']=False
            if defect=='not-killed':killed=False
            if defect=='not-exited':cleanup=False
            with self.subTest(defect=defect):
                self.assertFalse(qualify_fault('parent-death',changed,[],cleanup,killed,timing))

if __name__=='__main__':unittest.main()
