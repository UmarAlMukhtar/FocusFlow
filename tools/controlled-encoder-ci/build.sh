#!/bin/bash
set -euxo pipefail
C=$(cd -- "$(dirname -- "$0")" && pwd)
E=$(dirname "$C")
R=$(dirname "$(dirname "$E")")
M="$R/target/windows-verification/gnu-route/mingw/usr"
X="$M/bin/x86_64-w64-mingw32-"
test ! -e "$C/build/ffmpeg/config.h"
export PATH="$E/nasm-tool/prefix/usr/bin:$M/bin:/usr/bin:/bin" LC_ALL=C TZ=UTC SOURCE_DATE_EPOCH=0
export CC="${X}gcc" AR="${X}ar" RANLIB="${X}ranlib" STRIP="${X}strip"
unset CFLAGS CPPFLAGS CXXFLAGS LDFLAGS PKG_CONFIG_PATH LD_LIBRARY_PATH
export PKG_CONFIG_LIBDIR="$C/prefix/lib/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR=
export TMPDIR="$C/tmp"
mkdir -p "$C"/{sources,build/{zlib,x264,ffmpeg},prefix/{lib/pkgconfig,include},notices,receipts,tmp}
python3 - "$C" "$E" <<'PY'
import pathlib,json,hashlib,tarfile,shutil,sys
c,e=map(pathlib.Path,sys.argv[1:]);out=[]
for p in json.loads((e/'source-inputs/encoder-source-pins.json').read_text())['source_pins']:
 a=pathlib.Path(p['archive']);assert hashlib.sha256(a.read_bytes()).hexdigest()==p['sha256'];shutil.copy2(a,c/'sources'/a.name)
 with tarfile.open(a) as t:
  t.extractall(c/'sources',filter='data');files=[m for m in t.getmembers() if m.isfile()];root=files[0].name.split('/')[0];manifest=[]
  for f in files:
   h=hashlib.sha256(t.extractfile(f).read()).hexdigest();rel=pathlib.Path(f.name).relative_to(root)
   assert hashlib.sha256((c/'sources'/f.name).read_bytes()).hexdigest()==h
   assert hashlib.sha256((e/'source-inputs/sources'/root/rel).read_bytes()).hexdigest()==h
   manifest.append({'path':str(rel),'sha256':h,'bytes':f.size})
  (c/'receipts'/f'{p["name"]}-source-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
  out.append({'name':p['name'],'root':root,'archive_sha256':p['sha256'],'files':len(files),'patches':[]})
(c/'receipts/source-verification.json').write_text(json.dumps(out,indent=2)+'\n')
for f in ['encoder-source-pins.json','source-inventory.json','reused-gnu-rpm-verification.json','tool-availability.json']:shutil.copy2(e/'source-inputs'/f,c/'receipts'/f)
shutil.copy2(e/'nasm-tool/nasm-inputs.json',c/'receipts/nasm-inputs.json')
PY
{ uname -a; "$CC" --version; "${X}ld" --version; "$CC" -print-search-dirs; "$CC" -print-sysroot; make --version; pkg-config --version; nasm -v; printf '%s\n' "CC=$CC" "AR=$AR" "RANLIB=$RANLIB" "STRIP=$STRIP" "PATH=$PATH" "PKG_CONFIG_LIBDIR=$PKG_CONFIG_LIBDIR" "PKG_CONFIG_SYSROOT_DIR=$PKG_CONFIG_SYSROOT_DIR" "SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH" "TZ=$TZ" "LC_ALL=$LC_ALL" "TMPDIR=$TMPDIR"; } > "$C/receipts/environment-tools.txt"
printf 'int main(void){return 0;}\n' > "$C/build/cross-sanity.c"
"$CC" -v -static -static-libgcc -march=x86-64 -mtune=generic "$C/build/cross-sanity.c" -o "$C/build/cross-sanity.exe" > "$C/receipts/cross-sanity.log" 2>&1
"${X}objdump" -p "$C/build/cross-sanity.exe" > "$C/receipts/cross-sanity-pe.txt"
cp -a "$C/sources/zlib-1.3.2/." "$C/build/zlib/"
cd "$C/build/zlib"
make -f win32/Makefile.gcc -j2 PREFIX="$X" CFLAGS='-O2 -Wall -march=x86-64 -mtune=generic' libz.a
cp libz.a "$C/prefix/lib/"
cp zlib.h zconf.h "$C/prefix/include/"
cat > "$C/prefix/lib/pkgconfig/zlib.pc" <<EOF
prefix=$C/prefix
exec_prefix=\${prefix}
libdir=\${prefix}/lib
includedir=\${prefix}/include
Name: zlib
Description: private pinned static zlib
Version: 1.3.2
Libs: -L\${libdir} -lz
Cflags: -I\${includedir}
EOF
cd "$C/build/x264"
"$C/sources/x264-b35605ace3ddf7c1a5d67a2eb553f034aef41d55/configure" --host=x86_64-w64-mingw32 --cross-prefix="$X" --enable-static --disable-cli --disable-opencl --disable-avs --disable-swscale --disable-lavf --disable-ffms --disable-gpac --disable-lsmash --extra-cflags='-march=x86-64 -mtune=generic' --prefix="$C/prefix"
make -j2
make install
cd "$C/build/ffmpeg"
"$C/sources/ffmpeg-8.0.3/configure" --disable-autodetect --disable-everything --disable-network --disable-doc --disable-debug --disable-ffplay --disable-ffprobe --enable-ffmpeg --enable-static --disable-shared --enable-gpl --enable-libx264 --enable-zlib --enable-swscale --enable-swresample --pkg-config=/usr/bin/pkg-config --pkg-config-flags=--static --prefix="$C/prefix" --extra-cflags="-I$C/prefix/include -march=x86-64 -mtune=generic" --extra-ldflags="-L$C/prefix/lib -static -static-libgcc -Wl,-Map,$C/receipts/ffmpeg-link.map" --enable-encoder=libx264,aac,png,pcm_f32le --enable-decoder=h264,rawvideo,bmp,pam,wrapped_avframe,pcm_u8,pcm_s16le,pcm_s24le,pcm_s32le,pcm_f32le --enable-parser=h264 --enable-demuxer=mov,wav,image2,concat,rawvideo,pcm_f32le --enable-muxer=mp4,image2,framehash,null,pcm_f32le --enable-protocol=file,pipe --enable-bsf=setts,h264_mp4toannexb --enable-indev=lavfi,gdigrab --enable-filter=fps,tpad,trim,setpts,scale,color,format,geq,overlay,zoompan,setsar,concat,settb,gblur,loop,crop,asettb,atrim,asetpts,adelay,apad,anullsrc,asplit,aresample,aformat,amix,asetrate,afade --enable-cross-compile --target-os=mingw32 --arch=x86_64 --cpu=x86-64 --cross-prefix="$X" --disable-pthreads --enable-w32threads
make -j2 V=1
make install
sha256sum "$C/prefix/bin/ffmpeg.exe" > "$C/receipts/candidate.sha256"
