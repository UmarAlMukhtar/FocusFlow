"""Independent saved-media evidence; never emits a FocusFlow accepted clock."""
import argparse
from fractions import Fraction
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
    ident = 0
    for bit in range(16):
        color = patch(40 + bit * 32, 160)
        if all(c > 210 for c in color):
            ident |= 1 << bit
        elif not all(c < 40 for c in color):
            ident = None
            break
    return dict(id=ident, cornersValid=valid, cornersRgb=corners,
                rgbSha256=hashlib.sha256(rgb).hexdigest())


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


def summarize(events, probe, pixels, scenario):
    rows = [e for e in events if e['event'] == 'enqueued']
    starts = [e for e in events if e['event'] == 'startup']
    finishes = [e for e in events if e['event'] == 'finishComplete']
    require(len(starts) == len(finishes) == 1 and 0 < len(rows) <= CAP,
            'missing/duplicate startup, encoder finish, or bounded native frames')
    require(starts[0]['cursorRequest'] == 'WithoutCursor', 'cursor disable not requested')
    require(finishes[0]['enqueued'] == len(rows), 'incomplete enqueue receipts')
    require(any(e['event'] == 'captureStarted' for e in events), 'startup never completed')
    if scenario == 'close':
        require(any(e['event'] == 'sourceClosed' for e in events), 'source close not observed')
    if scenario == 'stall':
        stalls = [e for e in events if e['event'] in ('stallBegin', 'stallEnd')]
        require(len(stalls) == 2 and stalls[1]['qpc'] > stalls[0]['qpc'],
                'callback stall not observed')
    frequency = starts[0]['frequency']
    require(type(frequency) is int and frequency > 0, 'invalid QPC frequency')
    first = rows[0]['native100ns']
    for i, row in enumerate(rows):
        require(row['seq'] == i and (row['width'], row['height']) == (WIDTH, HEIGHT),
                'sequence gap or changed surface geometry')
        require(row['first100ns'] == first and row['delta100ns'] == row['native100ns']-first,
                'inconsistent native timestamp origin')
        require(row['native100ns'] >= 0 and (i == 0 or row['native100ns'] > rows[i-1]['native100ns']),
                'native timestamp regression or duplicate')
        require(row['qpcBefore'] <= row['qpcSubmitBefore'] <= row['qpcAfter'], 'QPC bracket reversed')
        require(row['qpcBefore100ns'] == row['qpcBefore']*10_000_000//frequency and
                row['qpcAfter100ns'] == row['qpcAfter']*10_000_000//frequency,
                'QPC frequency conversion mismatch')
        require(type(row['id']) is int, 'native marker undecodable')
    if scenario in ('animated', 'stall', 'close'):
        require(len({r['id'] for r in rows}) >= 3, 'animated source did not visibly progress')
    video, frames, packets, empty_audio = select_video(probe)
    timebase = Fraction(video['time_base'])
    require(timebase > 0, 'invalid saved time base')
    require(0 < len(frames) == len(pixels) == len(packets) <= CAP,
            'decoded/frame/packet count mismatch or cap exceeded')
    require(all(p['cornersValid'] and type(p['id']) is int for p in pixels),
            'decoded origin/color/ID marker validation failed')
    pts = [int(f['pts']) for f in frames]
    # Do not invent final-frame duration from nominal FPS or from enqueue count.
    durations = [int(f.get('duration', f.get('pkt_duration', 0))) for f in frames]
    require(all(d > 0 for d in durations), 'saved frame duration unavailable')
    require(all(b > a for a, b in zip(pts, pts[1:])), 'saved presentation PTS not strictly increasing')
    packet_pairs = sorted((int(p['pts']), int(p.get('duration', 0))) for p in packets)
    require(packet_pairs == list(zip(pts, durations)), 'packet/frame presentation timing disagrees')
    residual = [(p*timebase*10_000_000 - r['delta100ns']) for p, r in zip(pts, rows)]
    tolerance = timebase*10_000_000 + 1  # One saved tick + one native 100ns quantization unit.
    same_ids = [p['id'] == r['id'] for p, r in zip(pixels, rows)]
    observed = (len(rows) == len(frames) and pts[0] == 0 and all(same_ids)
                and all(abs(r) <= tolerance for r in residual))
    candidates = [[r['seq'] for r in rows if r['id'] == p['id']] for p in pixels]
    return dict(status='observed-preservation' if observed else 'resampled-or-unresolved',
                authorityAccepted=False, scenario=scenario, streamLayout=probe['streams'],
                videoStreamIndex=video['index'], emptyAudioStreams=empty_audio,
                enqueuedCount=len(rows), savedCount=len(frames), timeBase=str(timebase),
                savedPts=pts, savedDurations=durations,
                savedFirstSeconds=str(pts[0]*timebase),
                savedEndpointSeconds=str((pts[-1]+durations[-1])*timebase),
                nativeFirst100ns=first, nativeDeltas100ns=[r['delta100ns'] for r in rows],
                compositorPreStartup100ns=first-starts[0]['qpc']*10_000_000//frequency,
                ordinalResidual100ns=[str(r) for r in residual],
                comparisonTolerance100ns=str(tolerance), ordinalIdsEqual=same_ids,
                savedIds=[p['id'] for p in pixels], matchingNativeSequences=candidates,
                ambiguousIdFrames=sum(len(c) != 1 for c in candidates),
                savedPtsSteps=[b-a for a, b in zip(pts, pts[1:])],
                savedPresentationGaps=[b-a-d for a, b, d in zip(pts, pts[1:], durations)],
                warning='Observations only. Duplicate IDs cannot establish unique source identity; '
                        'no capture authority, GDI clock, physical input or audio timing is accepted.')


def analyze(directory, scenario, ffprobe, ffmpeg):
    directory = Path(directory)
    video = directory/'screen.mp4'
    raw = directory/'decode-temporary.rgb'
    commands = []
    result = dict(status='analysis-failed', authorityAccepted=False, scenario=scenario)

    def run(args, output, timeout=30):
        commands.append(args)
        with open(directory/(output+'.stderr'), 'wb') as errors, open(directory/output, 'wb') as dest:
            subprocess.run(args, stdout=dest, stderr=errors, check=True, timeout=timeout)

    try:
        require(0 < video.stat().st_size <= 64*1024*1024, 'video missing/empty or over 64 MiB budget')
        require((directory/'native.jsonl').stat().st_size < 2*1024*1024, 'native receipt exceeds budget')
        events = [json.loads(line) for line in (directory/'native.jsonl').read_text().splitlines()]
        rows = [e for e in events if e['event'] == 'enqueued']
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
        result.update(decodedCount=len(pixels), savedIds=[p['id'] for p in pixels],
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
        result.update(summarize(events, probe, pixels, scenario))
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
    raise SystemExit(0 if report['status'] == 'observed-preservation'
                     else 2 if report['status'] == 'analysis-failed' else 3)
