import argparse,pathlib,subprocess,json,hashlib,struct,wave,time,os
a=argparse.ArgumentParser();a.add_argument("--candidate",type=pathlib.Path,required=True);a.add_argument("--ffmpeg-reference",type=pathlib.Path,required=True);a.add_argument("--ffprobe-reference",type=pathlib.Path,required=True);a.add_argument("--output",type=pathlib.Path,required=True);args=a.parse_args()
if args.output.exists():raise SystemExit("Refusing existing evidence directory")
args.output.mkdir(parents=True)
if args.candidate.resolve() in [args.ffmpeg_reference.resolve(),args.ffprobe_reference.resolve()]:raise SystemExit("Candidate must be distinct from independent verifier")
c=args.output.resolve(); s=c/'smoke';s.mkdir(); binary=args.candidate.resolve(); records=[]
def run(name,args,stdin=None,good=True,exe=None):
 argv=[str(exe or binary),*map(str,args)];p=subprocess.run(argv,input=stdin,capture_output=True,timeout=45)
 (s/(name+'.stdout')).write_bytes(p.stdout);(s/(name+'.stderr')).write_bytes(p.stderr)
 records.append({'name':name,'argv':argv,'exit':p.returncode,'stdin_bytes':len(stdin or b''),'stdin_sha256':hashlib.sha256(stdin).hexdigest() if stdin else None})
 (s/'commands.json').write_text(json.dumps(records,indent=2)+'\n')
 assert (p.returncode==0)==good,(name,p.returncode,p.stderr.decode(errors='replace')[-3000:]);return p.stdout
base=['-hide_banner','-loglevel','error','-nostdin','-y']
source=s/'owned-padded.mp4'
run('version',['-version'])
expected=json.loads((pathlib.Path(__file__).parent/'provenance/feature-assertions.json').read_text())['exact_enabled_component_sets']
for kind,flag in [('ENCODER','encoders'),('DECODER','decoders'),('FILTER','filters'),('PROTOCOL','protocols'),('INDEV','devices')]:
 listing=run('startup-'+flag,['-hide_banner','-'+flag]).decode(errors='replace')
 for feature in expected[kind]:
  token=feature.lower()
  if kind=='ENCODER' and token=='libx264':token='libx264'
  assert any(token in line.split() for line in listing.splitlines()),(kind,token)

# Model the owned worker's existing zero padding: 159x89 content in 160x90 BGRA storage.
raw=bytearray()
for n in range(180):
 row=bytes([40,n,190,255])*159+bytes(4);raw.extend(row*89+bytes(160*4))
(s/'owned-padded.bgra').write_bytes(raw)
owned=['-hide_banner','-loglevel','error','-nostdin','-n','-f','rawvideo','-pixel_format','bgra','-video_size','160x90','-framerate','30','-i','pipe:0','-map','0:v:0','-an','-c:v','libx264','-preset','veryfast','-crf','23','-pix_fmt','yuv420p','-bf','0','-fps_mode','passthrough','-video_track_timescale','30000','-movflags','+faststart','-f','mp4',source]
run('owned-raw-bgra',owned,bytes(raw));del raw
verifier={}
for name in ['ffmpeg','ffprobe']:
 p=args.ffmpeg_reference if name=='ffmpeg' else args.ffprobe_reference;verifier[name]={'path':str(p),'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'version':subprocess.check_output([p,'-version'],text=True).splitlines()[0]}
(s/'independent-verifier.json').write_text(json.dumps(verifier,indent=2)+'\n')
def probe(name,path):return json.loads(run(name,['-v','error','-count_frames','-show_streams','-show_format','-of','json',path],exe=args.ffprobe_reference))
v=probe('owned-independent-probe',source)['streams'];assert len(v)==1 and v[0]['codec_name']=='h264' and v[0]['pix_fmt']=='yuv420p' and v[0]['nb_read_frames']=='180' and v[0]['avg_frame_rate']=='30/1' and v[0]['has_b_frames']==0 and v[0]['time_base']=='1/30000'
def inp(path,kind='mov'):return ['-protocol_whitelist','file,pipe','-f',kind,*(['-enable_drefs','0','-use_absolute_path','0'] if kind=='mov' else []),'-i',path]
for name,seek,mapping in [('first',[],['-map','0:v:0','-an','-c:v','copy']),('tail',['-sseof','-5'],['-map','0:v:0','-an','-c:v','copy']),('whole',[],['-map','0','-c','copy'])]:
 out=run('framehash-'+name,['-v','error','-copyts',*seek,*inp(source),*mapping,'-f','framehash','-hash','adler32','pipe:1']).decode();assert '#version: 2' in out and '#codec_id 0: h264' in out and '#extradata' in out
 rows=[[int(v.strip()) if j<5 else v.strip() for j,v in enumerate(line.split(','))] for line in out.splitlines() if line and not line.startswith('#')]
 assert rows and all(r[0]==0 and r[1]==r[2] and r[3]==1000 and len(r[5])==8 for r in rows)
 if name!='tail':assert len(rows)==180 and [r[2] for r in rows]==list(range(0,180000,1000))
 else:assert len(rows)>=150
run('independent-decoded-framehash',['-v','error','-i',source,'-f','framehash','-hash','sha256','pipe:1'],exe=args.ffmpeg_reference)
stats=run('clock-statistics',['-v','error','-f','lavfi','-i','color=size=16x16:rate=30','-frames:v','2','-copyts','-map','0:v:0','-vf','setpts=PTS-STARTPTS','-c:v','libx264','-threads','1','-stats_enc_pre:v:0','pipe:1','-stats_enc_pre_fmt:v:0','{fidx},{sidx},{n},{ni},{tbi},{ptsi},{tb},{pts}','-f','null','-']);assert len(stats.splitlines())==2
# Actual core patterns: indexed trim, PAM atlas loop/crop/overlay, zoompan d=1 and styled frame.
(s/'cursor-atlas.pam').write_bytes(b'P7\nWIDTH 8\nHEIGHT 8\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n'+bytes([255,0,0,255])*64)
graph="""[0:v]fps=30:round=up:eof_action=pass:start_time=0,tpad=stop_mode=clone:stop_duration=1,trim=start_pts=0:end_pts=30,setpts=PTS-STARTPTS[vc0];
[1:v]loop=loop=-1:size=1:start=0,setpts=N/(30*TB),crop=w=8:h=8:x='0':y='0':exact=1[cursor_tile];
[vc0]format=rgba[cursor_source];
[cursor_source][cursor_tile]overlay=x='20':y='20':enable='1':eval=frame:format=rgb:alpha=straight:shortest=1[cursor_composed];
[cursor_composed]scale=160:90,zoompan=z='1+0.1*on/30':x='clip(0.5*iw-iw/(2*zoom),0,iw-iw/zoom)':y='clip(0.5*ih-ih/(2*zoom),0,ih-ih/zoom)':d=1:s=160x90:fps=30,setsar=1[v0];
[v0]concat=n=1:v=1:a=0,settb=1/30,setpts=N[frame_input];
color=c=black:s=192x112:r=30:d=1,format=gbrp,geq=r='20+20*X/191':g='30':b='40'[frame_bg];
[frame_input]scale=160:90,format=gbrap,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='255*between(X,0,159)*between(Y,0,89)'[frame_content];
color=c=black@0:s=192x112:r=30:d=1,format=gbrap,geq=r=0:g=0:b=0:a='100*between(X,16,175)*between(Y,19,108)',gblur=sigma=6:planes=8[frame_shadow];
[frame_bg][frame_shadow]overlay=shortest=1:format=auto[frame_backdrop];
[frame_backdrop][frame_content]overlay=x=16:y=11:shortest=1:format=auto,format=yuv420p,setsar=1[v]"""
(s/'render.filter_complex.txt').write_text(graph)
common=[*base,'-copyts','-start_at_zero','-ss','0','-noaccurate_seek','-threads','1',*inp(source),'-f','image2','-framerate','30','-c:v','pam','-i',s/'cursor-atlas.pam','-filter_complex_threads','1','-filter_complex_script',s/'render.filter_complex.txt','-map','[v]','-an']
for i in [1,2]:
 run(f'cursor-frame-segment-{i}',[*common,'-frames:v','30','-c:v','libx264','-preset','veryfast','-crf','25','-pix_fmt','yuv420p','-bf','0','-flags','+cgop','-video_track_timescale','90000','-r','30',s/f'segment-{i}.mp4'])
run('still-png',[*common,'-frames:v','1','-c:v','png','-threads','1','-update','1',s/'preview.png']);assert (s/'preview.png').read_bytes().startswith(b'\x89PNG\r\n\x1a\n')
preview=probe('still-independent-probe',s/'preview.png')['streams'][0];assert (preview['width'],preview['height'])==(192,112)
(s/'cursor-concat.txt').write_text("file 'segment-1.mp4'\nfile 'segment-2.mp4'\n")
run('concat-setts',[*base,'-f','concat','-safe','1','-i',s/'cursor-concat.txt','-map','0:v','-an','-c:v','copy','-bsf:v','setts=pts=N*3000:dts=N*3000:duration=3000:time_base=1/90000','-video_track_timescale','90000','-movflags','+faststart',s/'cursor-video.mp4'])
cat=probe('concat-independent-probe',s/'cursor-video.mp4')['streams'][0];assert cat['nb_read_frames']=='60' and cat['duration']=='2.000000'
# Generate every WAV sample format accepted by hound; keep short/long forms for padding/trim.
for name,bits,tag in [('u8',8,1),('s16',16,1),('s24',24,1),('s32',32,1),('f32',32,3)]:
 for length in ([2400,144000] if name=='s16' else [48000]):
  sample={8:b'\x90',16:struct.pack('<h',4000),24:(4000).to_bytes(3,'little',signed=True),32:struct.pack('<f',.125) if tag==3 else struct.pack('<i',4000)}[bits];data=sample*length
  wav=struct.pack('<4sI4s4sIHHIIHH4sI',b'RIFF',36+len(data),b'WAVE',b'fmt ',16,tag,1,48000,48000*len(sample),len(sample),bits,b'data',len(data))+data
  path=s/f'{name}-{length}.wav';path.write_bytes(wav)
  audio='[1:a]asetpts=PTS-STARTPTS,adelay=100S:all=1,apad,atrim=end_sample=96000[aligned];[aligned]asplit=2[a0in][a1in];[a0in]asettb=1/48000,atrim=start_pts=0:end_pts=48000,asetpts=PTS-STARTPTS[a0];[a1in]asettb=1/48000,atrim=start_pts=48000:end_pts=96000,asetpts=PTS-STARTPTS[a1];[a0][a1]concat=n=2:v=0:a=1,asettb=1/48000,asetpts=N[a]'
  (s/'audio.filter_complex.txt').write_text(audio);out=s/f'audio-{name}-{length}.mp4'
  run(f'aac-{name}-{length}',[*base,*inp(s/'cursor-video.mp4'),*inp(path,'wav'),'-filter_complex_script',s/'audio.filter_complex.txt','-map','0:v','-map','[a]','-c:v','copy','-c:a','aac','-b:a','128k','-movflags','+faststart',out])
  streams=probe(f'aac-{name}-{length}-independent',out)['streams'];assert [x['codec_name'] for x in streams]==['h264','aac'];assert abs(float(streams[1]['duration'])-2)<.03
  run(f'aac-{name}-{length}-decode',['-v','error','-i',out,'-map','0:a','-f','null','-'],exe=args.ffmpeg_reference)
# FFmpeg returns a clean error for invalid WAV; application fallback requires the core's validator/runner.
for name,data in [('empty',b''),('corrupt',b'bad wave')]:
 p=s/f'{name}.wav';p.write_bytes(data)
 run(name+'-wav-rejected',[*base,*inp(p,'wav'),'-c:a','aac',s/(name+'.mp4')],good=False)
 run(name+'-explicit-video-fallback',[*base,*inp(s/'cursor-video.mp4'),'-map','0:v','-an','-c:v','copy',s/(name+'-fallback.mp4')]);assert len(probe(name+'-fallback-independent',s/(name+'-fallback.mp4'))['streams'])==1
run('missing-explicit-video-fallback',[*base,*inp(s/'cursor-video.mp4'),'-map','0:v','-an','-c:v','copy',s/'missing-fallback.mp4'])
# Real child kill/wait models the current core runner cancellation; no lifecycle integration claim.
argv=[str(binary),'-v','error','-re','-f','lavfi','-i','color=size=160x90:rate=30','-c:v','libx264','-f','null','-'];p=subprocess.Popen(argv,stdout=subprocess.PIPE,stderr=subprocess.PIPE);time.sleep(.25);assert p.poll() is None;p.kill();stdout,stderr=p.communicate(timeout=2);assert p.returncode!=0
records.append({'name':'cancellation-kill-reap','argv':argv,'exit':p.returncode});(s/'commands.json').write_text(json.dumps(records,indent=2)+'\n')
# Assert fixture outputs, exact executable identity, and retained results.
outputs=[{'path':p.relative_to(c).as_posix(),'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted(s.iterdir()) if p.is_file()]
(s/'output-manifest.json').write_text(json.dumps(outputs,indent=2)+'\n')
(s/'result.json').write_text(json.dumps({'status':'IN_PROGRESS','candidate_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'commands':len(records),'scope':'focused direct command/filter profiles derived from current consumers; no full product/native/MCP integration; WAV fallback explicitly exercised rather than attributed to app'},indent=2)+'\n')

# Independent decoded evidence must have the full expected frame count and changing image content.
rows=[line for line in (s/'independent-decoded-framehash.stdout').read_text().splitlines() if line and not line.startswith('#')];assert len(rows)==180 and len({r.rsplit(',',1)[1].strip() for r in rows})>30
raw=run('independent-first-frame-rgb',['-v','error','-i',source,'-frames:v','1','-f','rawvideo','-pix_fmt','rgb24','pipe:1'],exe=args.ffmpeg_reference);assert len(raw)==160*90*3
center=raw[(40*160+40)*3:(40*160+40)*3+3];corner=raw[-3:];assert center[0]>150 and center[1]<20 and center[2]<70 and max(corner)<100,(center,corner)
(s/'decoded-pixel-assertions.json').write_text(json.dumps({'frames':len(rows),'distinct_decoded_frame_hashes':len({r.rsplit(',',1)[1].strip() for r in rows}),'first_frame_center_rgb':list(center),'padded_corner_rgb':list(corner),'passed':True},indent=2)+'\n')
# Preview silence branch (used when the selected audio span lies outside available WAV samples).
(s/'silence.filter_complex.txt').write_text('anullsrc=r=48000:cl=1c,atrim=end_sample=48000,asetpts=PTS-STARTPTS[a]')
run('preview-silence-audio',[*base,*inp(s/'cursor-video.mp4'),'-filter_complex_script',s/'silence.filter_complex.txt','-map','0:v','-map','[a]','-c:v','copy','-c:a','aac','-b:a','128k',s/'preview-silence.mp4']);assert probe('preview-silence-independent',s/'preview-silence.mp4')['streams'][1]['codec_name']=='aac'
r=json.loads((s/'result.json').read_text());r['commands']=len(records);(s/'result.json').write_text(json.dumps(r,indent=2)+'\n');print('Independent decoded frame/pixel and preview silence assertions completed')
(s/'output-manifest.json').write_text(json.dumps([{'path':p.relative_to(c).as_posix(),'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted(s.iterdir()) if p.is_file() and p.name!='output-manifest.json'],indent=2)+'\n')

# Exact bounded first-packet command and preservation of unexpected audio in all-stream CFR probes.
first=run('exact-first-packet',['-v','error','-copyts',*inp(source),'-frames:v','1','-map','0:v:0','-an','-c:v','copy','-f','framehash','-hash','adler32','pipe:1']).decode()
rows=[r for r in first.splitlines() if r and not r.startswith('#')];assert len(rows)==1 and [int(v.strip()) for v in rows[0].split(',')[:4]]==[0,0,0,1000]
allstreams=run('all-stream-audio-visible',['-v','error','-copyts',*inp(s/'audio-s16-2400.mp4'),'-map','0','-c','copy','-f','framehash','-hash','adler32','pipe:1']).decode()
assert '#media_type 0: video' in allstreams and '#media_type 1: audio' in allstreams and any(r.startswith('1,') for r in allstreams.splitlines())
r=json.loads((s/'result.json').read_text());r['commands']=len(records);(s/'result.json').write_text(json.dumps(r,indent=2)+'\n');print('Exact bounded first-packet and all-stream audio visibility completed')
(s/'output-manifest.json').write_text(json.dumps([{'path':p.relative_to(c).as_posix(),'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted(s.iterdir()) if p.is_file() and p.name!='output-manifest.json'],indent=2)+'\n')
# Evidence metadata must describe the finalized result and every current smoke output.
manifest=json.loads((s/'output-manifest.json').read_text())
assert any(e['path']=='smoke/result.json' for e in manifest)
for entry in manifest:
 p=c/entry['path'];assert p.stat().st_size==entry['bytes'] and hashlib.sha256(p.read_bytes()).hexdigest()==entry['sha256'],entry['path']
assert json.loads((s/'result.json').read_text())['commands']==len(records)

# Final success is emitted only after every original and extra consumer check completes.
r=json.loads((s/'result.json').read_text())
r.update(status='PASS',commands=len(records),native_candidate_executed=True,scope='bounded generated candidate command/filter consumer profiles with independent decode/probe; explicit WAV fallback is a harness model; no Rust/Tauri/desktop capture/microphone/installer integration')
(s/'result.json').write_text(json.dumps(r,indent=2)+'\n')
(s/'output-manifest.json').write_text(json.dumps([{'path':p.relative_to(c).as_posix(),'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted(s.iterdir()) if p.is_file() and p.name!='output-manifest.json'],indent=2)+'\n')
print((s/'result.json').read_text())
