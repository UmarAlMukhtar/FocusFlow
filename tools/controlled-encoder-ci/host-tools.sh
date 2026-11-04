#!/bin/bash
set -euo pipefail
# Only disposable pinned Fedora CI containers may run this installation step.
recipe=$(cd -- "$(dirname -- "$0")" && pwd)
setup=$1
mkdir -p "$setup"
dnf -y --setopt=keepcache=True --disablerepo='*' --enablerepo=fedora --enablerepo=updates install python3 make pkgconf-pkg-config rpm-build cpio gnupg2 git diffutils findutils tar xz gawk sed grep 'libc.so.6()(64bit)' 'libgcc_s.so.1()(64bit)' 'libgmp.so.10()(64bit)' 'libm.so.6()(64bit)' 'libmpc.so.3()(64bit)' 'libmpfr.so.6()(64bit)' 'libstdc++.so.6()(64bit)' 'libz.so.1()(64bit)' > "$setup/host-install.log" 2>&1
python3 "$recipe/validate.py" --host
python3 - "$setup" <<'PY'
import hashlib,json,pathlib,shutil,subprocess,sys
out=pathlib.Path(sys.argv[1])
query='%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}|%{SOURCERPM}|%{SHA256HEADER}|%{PAYLOADSHA256}\n'
rows=subprocess.check_output(['rpm','-qa','--qf',query],text=True,timeout=30).splitlines()
(out/'host-rpms.txt').write_text('\n'.join(sorted(rows))+'\n')
def sha(path):
 h=hashlib.sha256()
 with path.open('rb') as source:
  while block:=source.read(1024*1024):h.update(block)
 return h.hexdigest()
cache=[]
for path in sorted(pathlib.Path('/var/cache').rglob('*.rpm')):
 cache.append({'name':path.name,'bytes':path.stat().st_size,'sha256':sha(path),'identity':subprocess.check_output(['rpm','-qp','--qf',query,str(path)],text=True,timeout=30).strip()})
(out/'host-cached-rpms.json').write_text(json.dumps(cache,indent=2)+'\n')
tools=[]
for name in ['bash','python3','make','pkg-config','rpm','rpmkeys','rpm2cpio','cpio','gpg','git','awk','sed','grep','tar','xz']:
 path=pathlib.Path(shutil.which(name)).resolve()
 tools.append({'name':name,'path':str(path),'sha256':sha(path),'rpm':subprocess.check_output(['rpm','-qf','--qf',query,str(path)],text=True,timeout=30).strip()})
(out/'host-executables.json').write_text(json.dumps(tools,indent=2)+'\n')
(out/'os-release.txt').write_text(pathlib.Path('/etc/os-release').read_text())
PY
