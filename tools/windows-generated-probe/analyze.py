"""Independent saved-media evidence; never emits a FocusFlow accepted clock."""
import argparse
from fractions import Fraction
from math import gcd
import hashlib
import json
from pathlib import Path
import subprocess

WIDTH, HEIGHT, CAP = 640, 360, 600


def require(condition, message):
    if not condition:
        raise ValueError(message)


def decode(rgb):
    require(len(rgb) == WIDTH * HEIGHT * 3, 'truncated or wrong-size RGB frame')

    def patch(x, y):
        return [sum(rgb[((y + dy) * WIDTH + x + dx) * 3 + c]
                    for dy in range(-3, 4) for dx in range(-3, 4)) / 49
                for c in range(3)]

    corners = [patch(x, y) for x, y in [(16, 16), (624, 16), (16, 344), (624, 344)]]
    expected = [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0)]
    valid = all(abs(a-b) <= 60 for color, target in zip(corners, expected)
                for a, b in zip(color, target))
    def marker(y):
        ident = 0
        for bit in range(16):
            color = patch(40 + bit * 32, y)
            if all(c > 210 for c in color):
                ident |= 1 << bit
            elif not all(c < 40 for c in color):
                return None
        return ident
    return dict(id=marker(160), sourceTag=marker(240), cornersValid=valid,
                cornersRgb=corners, rgbSha256=hashlib.sha256(rgb).hexdigest())


def select_video(probe):
    streams = probe['streams']
    require(isinstance(streams, list) and streams, 'missing stream layout')
    by_index = {}
    for stream in streams:
        require(isinstance(stream, dict) and type(stream.get('index')) is int
                and stream['index'] >= 0, 'invalid stream index')
        require(stream['index'] not in by_index, 'duplicate stream index')
        require(stream.get('codec_type') in ('video', 'audio'), 'unexpected stream kind')
        by_index[stream['index']] = stream
    videos = [s for s in streams if s['codec_type'] == 'video']
    require(len(videos) == 1, 'expected exactly one video stream')
    video = videos[0]
    require((video.get('width'), video.get('height')) == (WIDTH, HEIGHT), 'saved geometry mismatch')
    for section in ('frames', 'packets'):
        require(isinstance(probe[section], list), 'invalid '+section+' evidence')
        for row in probe[section]:
            require(isinstance(row, dict) and type(row.get('stream_index')) is int
                    and row['stream_index'] in by_index, 'unknown or missing '+section+' stream association')
            kind = by_index[row['stream_index']]['codec_type']
            if section == 'frames':
                require(row.get('media_type') == kind, 'frame media type disagrees with stream')
            require(kind != 'audio', 'unexpected audio payload in '+section)
    frames = [f for f in probe['frames'] if f['stream_index'] == video['index']]
    packets = [r for r in probe['packets'] if r['stream_index'] == video['index']]
    require(0 < len(frames) <= CAP and len(packets) <= CAP, 'saved video exceeds frame budget or is empty')
    return video, frames, packets, [s['index'] for s in streams if s['codec_type'] == 'audio']


def summarize(events, probe, pixels, scenario, ledger):
    def one(name):
        found = [e for e in events if e['event'] == name]
        require(len(found) == 1, 'missing/duplicate '+name)
        return found[0]
    start, admission, terminal = one('startup'), one('admission'), one('terminal')
    one('captureStarted')
    require(one('encoderOwned')['inJob'] is True, 'encoder ownership unproved')
    require(start['cursorRequest'] == 'WithoutCursor', 'cursor disable not requested')
    f = start['frequency']
    require(type(f) is int and f > 0, 'invalid QPC frequency')
    a = Fraction(admission['qpc'], f)
    rows = [e for e in events if e['event'] == 'snapshot']
    require(0 < len(rows) <= CAP, 'missing or excessive source rows')
    admitted = []
    for i, row in enumerate(rows):
        require(row['seq'] == i and (row['width'],row['height']) == (WIDTH,HEIGHT),
                'sequence gap or changed geometry')
        t = row['native100ns']
        require(type(t) is int and t >= 0 and (i == 0 or t >= rows[i-1]['native100ns']),
                'negative native timestamp or regression')
        require(type(row['id']) is int and 0 <= row['id'] < 600 and row['cornersValid'] is True,
                'native marker/corners invalid')
        require(0 <= row['callbackQpc'] <= row['copyStartQpc'] <= row['copyEndQpc'] <= row['copyCompleteQpc'],
                'copy QPC bracket reversed')
        require(i == 0 or rows[i-1]['copyCompleteQpc'] <= row['callbackQpc'], 'callback order reversed')
        pre = Fraction(t,10_000_000) < a
        require(row['preRoll'] is pre, 'pre-roll classification mismatch')
        if not pre: admitted.append(row)
    require(admitted, 'no post-admission source')
    epoch = admitted[0]['native100ns']
    require(terminal['epoch100ns'] == epoch, 'first admitted epoch mismatch')
    stops = [e for e in events if e['event'] in ('stopInstalled','sourceClosed')]
    require(stops, 'missing observed cutoff')
    for r in stops:
        if r['event'] == 'stopInstalled':
            require(r['reason'] in ('stop','close') and r['requestQpc'] <= r['qpc'], 'invalid explicit stop request')
    cutoff = min(r['qpc'] for r in stops)
    require(terminal['stopQpc'] == cutoff, 'cutoff differs from earliest observation')
    end = Fraction(cutoff,f)-Fraction(epoch,10_000_000)
    n = max(0, (30*end).__floor__())
    require(0 < n <= CAP and terminal['expectedCells'] == terminal['completeWrites'] == n,
            'complete-cell/write count mismatch')
    require(terminal['ffmpegExit'] == 0 and terminal['reaped'] is True and terminal['writerDone'] is True
            and terminal['failure'] is None, 'encoder/writer finalization failed')
    for key,limit in [('sourceQueueHigh',8),('futureQueueHigh',8),('outputQueueHigh',2),('ownedHigh',22)]:
        require(type(terminal[key]) is int and 0 <= terminal[key] <= limit, 'bounded queue/ownership limit')
    if scenario == 'close':
        close = one('sourceClosed')
        stop = one('stopInstalled')
        require(stop['reason'] == 'close' and stop['qpc'] <= close['qpc'], 'close without prior installed Stop')
    if scenario == 'stall':
        require(one('stallEnd')['qpc']-one('stallBegin')['qpc'] >= f*350//1000, '350ms callback stall not observed')
    if scenario in ('animated','stall','close'):
        require(len({r['id'] for r in admitted}) >= 3, 'animated source did not progress')
    selected = [r for r in ledger if r['event'] == 'selected']
    written = [r for r in ledger if r['event'] == 'written']
    require(len(ledger) == 2*n and len(selected) == len(written) == n, 'missing or extra selection/write rows')
    hold, delay, write_delay = [], [], []
    late = 0
    for i,(r,w) in enumerate(zip(selected,written)):
        require(r['ordinal'] == w['ordinal'] == i, 'output ordinal gap/duplicate')
        target = Fraction(epoch,10_000_000)+Fraction(i,30)
        require(r['targetNumerator'] == epoch*30+i*10_000_000 and r['targetDenominator'] == 300_000_000,
                'exact target grid mismatch')
        decision = Fraction(r['decisionQpc'],f)
        require(decision >= target+Fraction(1,30), 'decision before complete cell end')
        require(i == 0 or r['decisionQpc'] >= selected[i-1]['decisionQpc'], 'decisions reordered')
        frontier = r['deliveredThroughSeq']
        require(type(frontier) is int and 0 <= frontier < len(rows), 'invalid delivered frontier')
        require(all(x['copyCompleteQpc'] <= r['decisionQpc'] for x in rows if x['seq'] <= frontier)
                and all(x['seq'] <= frontier for x in rows if x['copyCompleteQpc'] < r['decisionQpc']),
                'delivery frontier disagrees with synchronized publication')
        require(i == 0 or frontier >= selected[i-1]['deliveredThroughSeq'], 'delivery frontier regressed')
        available = [x for x in admitted if x['seq'] <= frontier and Fraction(x['native100ns'],10_000_000) <= target
                     and x['copyCompleteQpc'] <= r['decisionQpc']]
        require(available, 'no causal snapshot')
        expected = max(available,key=lambda x:(x['native100ns'],x['seq']))
        require((r['sourceSeq'],r['sourceNative100ns'],r['sourceId']) ==
                (expected['seq'],expected['native100ns'],expected['id']), 'selection is not greatest causal snapshot')
        require(w['bytesWritten'] == WIDTH*HEIGHT*4 and r['decisionQpc'] <= w['writeBeginQpc'] <= w['writeEndQpc'],
                'incomplete write or reversed write bracket')
        require(i == 0 or written[i-1]['writeEndQpc'] <= w['writeBeginQpc'], 'overlapping writer frames')
        hold.append(target-Fraction(expected['native100ns'],10_000_000))
        delay.append(decision-target-Fraction(1,30))
        write_delay.append(Fraction(w['writeEndQpc']-w['writeBeginQpc'],f))
        late += sum((x['native100ns'],x['seq']) > (expected['native100ns'],expected['seq']) and Fraction(x['native100ns'],10_000_000) <= target
                    and (x['copyCompleteQpc'] > r['decisionQpc'] or x['seq'] > frontier) for x in admitted)
    video, frames, packets, audio = select_video(probe)
    require(not audio, 'audio stream is forbidden in owned feed')
    timebase = Fraction(video['time_base'])
    require(timebase > 0 and len(frames) == len(packets) == len(pixels) == n, 'saved/frame/packet/write count mismatch')
    pts = [int(r['pts']) for r in frames]
    durations = [int(r.get('duration',r.get('pkt_duration',0))) for r in frames]
    for i,(p,d,pixel,r) in enumerate(zip(pts,durations,pixels,selected)):
        require(p*timebase == Fraction(i,30) and d*timebase == Fraction(1,30), 'saved PTS/duration not exact grid')
        require(pixel['cornersValid'] is True and pixel['sourceTag'] == r['sourceSeq'] and pixel['id'] == r['sourceId'],
                'saved tag/original ID/corner mismatch')
    require(sorted((int(r['pts']),int(r.get('duration',0))) for r in packets) == list(zip(pts,durations)),
            'packet/frame presentation timing disagrees')
    endpoint = (pts[-1]+durations[-1])*timebase
    require(endpoint == Fraction(n,30), 'file-derived endpoint mismatch')
    steps = [b['native100ns']-a['native100ns'] for a,b in zip(rows,rows[1:])]
    callback_offsets = [Fraction(r['callbackQpc'],f)-Fraction(r['native100ns'],10_000_000) for r in rows]
    return dict(status='owned-feed-verified', authorityAccepted=False, scenario=scenario,
                mappingStatus='verified-all-saved-frames', lifecycleStatus='finalized',
                latencyStatus='measured-no-accuracy-bound-claimed', savedCount=n,
                nativeTimestampDuplicateCount=steps.count(0),
                nativePositiveStepGcd100ns=gcd(*(step for step in steps if step > 0)),
                nativeCount=len(rows), admittedCount=len(admitted), preRollCount=len(rows)-len(admitted),
                savedPts=pts,savedDurations=durations,timeBase=str(timebase),
                savedFirstSeconds=str(pts[0]*timebase),savedEndpointSeconds=str(endpoint),
                savedIds=[p['id'] for p in pixels],savedTags=[p['sourceTag'] for p in pixels],
                lateObservations=late, fractionalCellDiscardSeconds=str(end-Fraction(n,30)),
                maxHoldSeconds=str(max(hold)),maxSchedulerDelaySeconds=str(max(delay)),
                maxWriteSeconds=str(max(write_delay)),
                minCallbackDelaySeconds=str(min(callback_offsets)),
                maxCallbackDelaySeconds=str(max(callback_offsets)),
                maxCopySeconds=str(max(Fraction(r['copyEndQpc']-r['copyStartQpc'],f) for r in rows)),
                maxCopyCompleteDelaySeconds=str(max(Fraction(r['copyCompleteQpc'],f)-Fraction(r['native100ns'],10_000_000) for r in rows)),
                warning='Delivered snapshots only; 100ns is the native TimeSpan unit, not measured resolution or accuracy. Positive-step GCD is observational only (zero means no positive step). No unseen compositor, physical display/input, cursor or audio timing claim.')


def analyze(directory, scenario, ffprobe, ffmpeg):
    directory = Path(directory)
    video = directory/'screen.mp4'
    raw = directory/'decode-temporary.rgb'
    commands = []
    result = dict(status='analysis-failed', authorityAccepted=False, scenario=scenario, mappingStatus='unqualified', lifecycleStatus='unqualified', latencyStatus='unqualified')

    def run(args, output, timeout=30):
        commands.append(args)
        with open(directory/(output+'.stderr'), 'wb') as errors, open(directory/output, 'wb') as dest:
            subprocess.run(args, stdout=dest, stderr=errors, check=True, timeout=timeout)

    try:
        require(0 < video.stat().st_size <= 64*1024*1024, 'video missing/empty or over 64 MiB budget')
        require((directory/'native.jsonl').stat().st_size < 2*1024*1024, 'native receipt exceeds budget')
        events = [json.loads(line) for line in (directory/'native.jsonl').read_text().splitlines()]
        rows = [e for e in events if e['event'] == 'snapshot']
        result.update(videoSha256=hashlib.sha256(video.read_bytes()).hexdigest(),
                      videoBytes=video.stat().st_size, enqueuedCount=len(rows), nativeFrames=rows,
                      nativeUndecodableSequences=[r['seq'] for r in rows if type(r.get('id')) is not int])
        # Inspect all streams and their frames/packets; a declared audio track is not sample evidence.
        run([ffprobe, '-v', 'error', '-show_streams', '-show_frames', '-show_entries',
             'frame=media_type,stream_index,pts,duration,pkt_duration,nb_samples',
             '-of', 'json', str(video)], 'frames.json')
        probe = json.loads((directory/'frames.json').read_text())
        result.update(streamLayout=probe.get('streams'), probedFrames=probe.get('frames'))
        run([ffprobe, '-v', 'error', '-show_packets', '-show_entries',
             'packet=stream_index,pts,duration,size', '-of', 'json', str(video)], 'packets.json')
        probe.update(json.loads((directory/'packets.json').read_text()))
        result['probedPackets'] = probe.get('packets')
        track, frames, packets, empty_audio = select_video(probe)
        result.update(videoStreamIndex=track['index'], emptyAudioStreams=empty_audio,
                      streamAssociation=[dict(index=s['index'], codecType=s['codec_type'],
                          frameCount=sum(f['stream_index'] == s['index'] for f in probe['frames']),
                          packetCount=sum(r['stream_index'] == s['index'] for r in probe['packets']))
                          for s in probe['streams']],
                      savedCount=len(frames), savedCountMatchesEnqueued=len(frames) == len(rows),
                      savedPts=[f.get('pts') for f in frames], timeBase=track.get('time_base'),
                      savedDurations=[f.get('duration', f.get('pkt_duration', 0)) for f in frames])
        # Explicit index, passthrough PTS/multiplicity, no -r, scale or fps filter.
        run([ffmpeg, '-nostdin', '-v', 'error', '-i', str(video), '-map', '0:'+str(track['index']),
             '-an', '-frames:v', str(CAP+1), '-fps_mode', 'passthrough',
             '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-'], raw.name)
        size = WIDTH*HEIGHT*3
        require(raw.stat().st_size % size == 0 and 0 < raw.stat().st_size//size <= CAP,
                'invalid or excessive decoded frame count')
        pixels = []
        with raw.open('rb') as stream:
            while frame := stream.read(size):
                pixels.append(decode(frame))
        (directory/'decoded-markers.json').write_text(json.dumps(pixels, indent=2)+'\n')
        result.update(decodedCount=len(pixels), savedIds=[p['id'] for p in pixels], savedTags=[p['sourceTag'] for p in pixels],
                      decodedInvalidCornerFrames=[i for i, p in enumerate(pixels) if not p['cornersValid']],
                      decodedUndecodableFrames=[i for i, p in enumerate(pixels) if type(p['id']) is not int])
        # Retain a measured endpoint even if a subsequent color/native-ID check rejects the mapping.
        try:
            timebase = Fraction(track['time_base'])
            duration = int(result['savedDurations'][-1])
            if timebase > 0 and duration > 0:
                result['savedEndpointSeconds'] = str((int(frames[-1]['pts'])+duration)*timebase)
        except (ValueError, TypeError, KeyError, ZeroDivisionError):
            pass  # summarize still rejects invalid/missing timing; no guessed endpoint is emitted.
        require((directory/'selection.jsonl').stat().st_size < 2*1024*1024, 'selection receipt exceeds budget')
        ledger = [json.loads(line) for line in (directory/'selection.jsonl').read_text().splitlines()]
        result.update(summarize(events, probe, pixels, scenario, ledger))
    except (OSError, ValueError, KeyError, TypeError, ZeroDivisionError, subprocess.SubprocessError) as error:
        result['error'] = str(error)
    finally:
        raw.unlink(missing_ok=True)
        (directory/'analysis-commands.json').write_text(json.dumps(commands, indent=2)+'\n')
        (directory/'analysis.json').write_text(json.dumps(result, indent=2)+'\n')
    # Returning a failed classification keeps the existing runner from overwriting these receipts.
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('scenario', choices=['first', 'static', 'animated', 'stall', 'close'])
    parser.add_argument('directory')
    parser.add_argument('--ffprobe', required=True)
    parser.add_argument('--ffmpeg', required=True)
    args = parser.parse_args()
    report = analyze(args.directory, args.scenario, args.ffprobe, args.ffmpeg)
    print(json.dumps(report))
    raise SystemExit(0 if report['status'] == 'owned-feed-verified'
                     else 2 if report['status'] == 'analysis-failed' else 3)
