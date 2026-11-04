import pathlib,json,hashlib,re,subprocess,shutil
from source_coverage import verify_profile_coverage
c=pathlib.Path(__file__).resolve().parent;e=c.parent;r=e.parent.parent;m=r/'target/windows-verification/gnu-route/mingw/usr';out=c/'receipts';binary=c/'prefix/bin/ffmpeg.exe'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def save(n,x):(out/n).write_text(json.dumps(x,indent=2)+'\n')
def cmd(args,n):
 p=subprocess.run(list(map(str,args)),capture_output=True,text=True);(out/n).write_text(p.stdout+p.stderr);assert p.returncode==0,(n,p.returncode);return p.stdout
config=(c/'build/ffmpeg/config_components.h').read_text(); general=(c/'build/ffmpeg/config.h').read_text();linux=(c/'receipts/linux-config_components.h').read_text()
kinds=['ENCODER','DECODER','PARSER','DEMUXER','MUXER','PROTOCOL','BSF','INDEV','OUTDEV','FILTER','HWACCEL']
sets={}
for kind in kinds:
 enabled=lambda x:sorted(re.findall(r'^#define CONFIG_(\w+)_'+kind+r' 1$',x,re.M))
 expected=enabled(linux) if kind!='INDEV' else ['GDIGRAB','LAVFI']
 additions={'FILTER':['AMIX','ASETRATE','AFADE'],'ENCODER':['PCM_F32LE'],'DEMUXER':['PCM_F32LE'],'MUXER':['PCM_F32LE']}
 expected=sorted(set(expected)|set(additions.get(kind,[])))
 actual=enabled(config);assert actual==expected,(kind,actual,expected);sets[kind]=actual
for token in ['CONFIG_NETWORK','CONFIG_NONFREE','CONFIG_VERSION3','CONFIG_SHARED','HAVE_PTHREADS']:
 assert f'#define {token} 0' in general,token
for token in ['CONFIG_GPL','CONFIG_LIBX264','CONFIG_ZLIB','CONFIG_RUNTIME_CPUDETECT','HAVE_W32THREADS','ARCH_X86_64']:
 assert f'#define {token} 1' in general,token
x=(c/'build/x264/config.h').read_text()
for token in ['HAVE_WIN32THREAD','HAVE_THREAD','ARCH_X86_64','HAVE_MMX','HAVE_BITDEPTH8']:
 assert f'#define {token} 1' in x,token
assert '#define HAVE_POSIXTHREAD 0' in x
flags=(c/'build/ffmpeg/ffbuild/config.mak').read_text()+(c/'build/x264/config.mak').read_text()
assert not re.search(r'-march=native|-mtune=native',flags)
assert '-march=x86-64 -mtune=generic' in flags
# config component tables + linked component symbols substantiate static allowlist (no PE executed).
nm=cmd([m/'bin/x86_64-w64-mingw32-nm',c/'build/ffmpeg/ffmpeg_g.exe'],'pe-symbols.txt')
for symbol in ['ff_libx264_encoder','ff_aac_encoder','ff_png_encoder','ff_rawvideo_demuxer','ff_gdigrab_demuxer','ff_h264_decoder','ff_vf_format','x264_encoder_open_165','ff_af_amix','ff_af_asetrate','ff_af_afade','ff_pcm_f32le_encoder','ff_pcm_f32le_demuxer','ff_pcm_f32le_muxer']:
 assert symbol in nm,symbol
pe=cmd([m/'bin/x86_64-w64-mingw32-objdump','-p',binary],'pe-private-header.txt')
headers=cmd([m/'bin/x86_64-w64-mingw32-objdump','-f',binary],'pe-file-header.txt')
assert 'pei-x86-64' in headers
imports=sorted(re.findall(r'DLL Name: (\S+)',pe));assert imports, 'no PE imports parsed'
allowed={'GDI32.dll','KERNEL32.dll','msvcrt.dll','SHELL32.dll','USER32.dll','bcrypt.dll'}
# OS imports are inspected individually; compiler vendor DLLs must not leak into closure.
assert not any(re.search(r'libgcc|libstdc|libwinpthread|zlib|x264',s,re.I) for s in imports),imports
assert set(s.lower() for s in imports)<=set(s.lower() for s in allowed),imports
linkmap=(out/'ffmpeg-link.map').read_text();loads=re.findall(r'^LOAD (.+)$',linkmap,re.M)
inputs=[]
rpm_files={q['path']:q['rpm'] for q in json.loads((out/'rpm-payload-verification.json').read_text())}
for name in loads:
 # MinGW ld synthesizes PE export/relocation sections under this internal label.
 if name=='dll stuff':continue
 p=pathlib.Path(name)
 if not p.is_absolute():p=c/'build/ffmpeg'/p
 p=p.resolve()
 assert p.is_relative_to(c) or p.is_relative_to(m),(name,str(p))
 assert p.is_file(),str(p)
 inputs.append({'path':str(p),'bytes':p.stat().st_size,'sha256':sha(p),'source':rpm_files.get(str(p.relative_to(m.parent))) if p.is_relative_to(m) else 'owned candidate build'})
for name in ['libx264.a','libz.a']:
 p=c/'prefix/lib'/name;assert str(p) in linkmap,name
 assert any(t['path']==str(p) for t in inputs),name
assert 'libz.dll' not in linkmap and 'libx264.dll' not in linkmap
# Include only runtime archive members actually pulled; LOAD lists also include unused/as-needed inputs.
archives=sorted(set(re.findall(r'([^\s()]+\.a)\(([^)]+)\)',linkmap)))
save('link-inputs.json',{'files':inputs,'linker_synthetic_labels':[x for x in loads if x=='dll stuff']})
save('archive-members.json',[{'archive':a,'member':b} for a,b in archives])
save('pe-imports.json',{'format':'pei-x86-64','imports':imports,'executed':False,'os_resolution_tested':False,'compiler_runtime_dlls':[]})
save('feature-assertions.json',{'all_assertions_passed':True,'method':'static generated config and linked unstripped PE symbols; no executable run','exact_enabled_component_sets':sets,'baseline':'accepted Linux candidate ee4cbd38b7ecf22071a93596f3521b06bb35cc4ededc9a85dfa105e29465180a; replace XCBGRAB with GDIGRAB; explicit audio profile additions','runtime_cpu_dispatch':True,'generic_x86_64_c_flags':True,'x264_win32_threads':True,'ffmpeg_win32_threads':True,'network':False,'audio_profile_additions':additions,'mapper_algorithm_qualified':False})
for name,src in [('ffmpeg-config.h','build/ffmpeg/config.h'),('ffmpeg-config_components.h','build/ffmpeg/config_components.h'),('ffmpeg-config.mak','build/ffmpeg/ffbuild/config.mak'),('ffmpeg-config.log','build/ffmpeg/ffbuild/config.log'),('x264-config.h','build/x264/config.h'),('x264-config.mak','build/x264/config.mak'),('x264-config.log','build/x264/config.log')]:shutil.copy2(c/src,out/name)
for name,root,patterns in [('ffmpeg',c/'sources/ffmpeg-8.0.3',['COPYING*','LICENSE.md']),('x264',c/'sources/x264-b35605ace3ddf7c1a5d67a2eb553f034aef41d55',['COPYING']),('zlib',c/'sources/zlib-1.3.2',['LICENSE'])]:
 dest=c/'notices'/name;dest.mkdir(exist_ok=True)
 for pat in patterns:
  for p in root.glob(pat):shutil.copy2(p,dest/p.name)
for name in ['mingw64-crt','mingw64-headers','mingw64-libgcc','mingw64-gcc','mingw64-winpthreads']:
 src=m/'share/licenses'/name;shutil.copytree(src,c/'notices'/name,dirs_exist_ok=True)
rpms=json.loads((out/'reused-gnu-rpm-verification.json').read_text())['packages'];inventory=[]
for p in rpms:
 q=cmd(['rpm','-qp','--qf','%{NAME}|%{VERSION}-%{RELEASE}|%{SOURCERPM}|%{LICENSE}\n',p['path']],'rpm-'+pathlib.Path(p['path']).stem+'.txt')
 inventory.append({'rpm':pathlib.Path(p['path']).name,'sha256':p['sha256'],'metadata':q.strip(),'source_archive_available_in_candidate':False})
save('compiler-runtime-source-inventory.json',inventory)
save('static-runtime-members.json',{name:sorted({b for a,b in archives if pathlib.Path(a).name==name}) for name in ['libpthread.a','libgcc.a','libgcc_eh.a','libatomic.a','libmingw32.a','libmingwex.a','libmsvcrt.a']})
save('candidate.json',{'path':str(binary),'sha256':sha(binary),'bytes':binary.stat().st_size,'build_exit':0,'profile_assertions':True,'runtime_tests':False,'native_windows_qualified':False,'source_delivery_and_notices_complete':False,'static_private_libx264':sha(c/'prefix/lib/libx264.a'),'static_private_zlib':sha(c/'prefix/lib/libz.a'),'imports':imports,'limits':['no Windows/Wine/PE execution','no native consumer, capture, installer or clean-machine DLL resolution tests','corresponding-source delivery for compiler/CRT components and notices legal assessment incomplete','no bit-identical rebuild claim']})
print('Static PE/config/link/feature assertions passed:',binary,sha(binary),imports)

# Exact target RPMs keep the retained source mappings applicable only if selection matches.
coverage=json.loads((out/'coverage-397.json').read_text())['entries']
def resolved_link_input(name):
 path=pathlib.Path(name)
 return (path if path.is_absolute() else c/'build/ffmpeg'/path).resolve()
actual_members={(pathlib.Path(archive).name,member) for archive,member in archives if resolved_link_input(archive).is_relative_to(m)}
extra_archive=m/'x86_64-w64-mingw32/sys-root/mingw/lib/libmsvcrt.a'
extra_member=subprocess.check_output([m/'bin/x86_64-w64-mingw32-ar','p',extra_archive,'lib64_libmsvcrt_common_a-cbrt.o'],timeout=30)
profile_coverage=verify_profile_coverage(out,actual_members,extra_archive,extra_member)
for item in coverage:
 if item['kind']=='explicit startup object':
  matches=[entry for entry in inputs if pathlib.Path(entry['path']).name==item['member']]
  assert len(matches)==1 and matches[0]['sha256']==item['object_sha256'],item['member']
shutil.copytree(out/'audio-profile-extra/notices',c/'notices/audio-profile-extra',dirs_exist_ok=True)
save('target-source-map-check.json',{**profile_coverage,'startup_objects':3,'exact_pinned_target_rpms':True})
