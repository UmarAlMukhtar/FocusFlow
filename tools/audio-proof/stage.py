"""Fixed hosted build observations and same-job discovery; never publishes approval."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import shutil
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid
import zipfile

REPOSITORY = 'UmarAlMukhtar/FocusFlow'
SOURCE_REF = 'refs/heads/codex-audio-native-qualification'
WORKFLOW_REF = REPOSITORY + '/.github/workflows/windows-audio-native-qualification.yml@' + SOURCE_REF
WORKER = '.superpowers/sdd/2026-11-04-windows-audio-worker'
STAGE = '.audio-native-stage'
TARGET = '.audio-native-target'
CARGO = '.audio-native-cargo'
TOTAL_CAP = 1024 * 1024 * 1024
FILE_CAP = 512 * 1024 * 1024
BUILD_FILES = ['build-request.json', 'approval-request.json', 'source-input-map.json',
               'compiler-input-map.json', 'tool-identities.json', 'source-files.zip',
               'private_audio_proof.exe', 'build-artifacts.json']
COMMANDS = ['rustup-install', 'rustc-version', 'cargo-version', 'sysroot', 'vswhere',
            'cargo-build', 'git-head']
VS_COMPARISONS = [('vswhere-programdata', ('ProgramData',)),
                  ('vswhere-allusersprofile', ('ALLUSERSPROFILE',)),
                  ('vswhere-commondata', ('ProgramData', 'ALLUSERSPROFILE')),
                  ('vswhere-recheck', ())]
VS_FAILURE_FILES = [label + suffix for label, _ in VS_COMPARISONS
                    for suffix in ['.log', '.json']] + ['vswhere-summary.json']


def encoded(value):
    return (json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True) + '\n').encode()


def safe_path(root, relative):
    if not isinstance(relative, str) or not relative or '\\' in relative or ':' in relative:
        raise ValueError('Non-normal artifact path')
    parts = PurePosixPath(relative).parts
    if PurePosixPath(relative).is_absolute() or any(p in ('', '.', '..') for p in parts) or '/'.join(parts) != relative:
        raise ValueError('Non-normal artifact path')
    path = root.joinpath(*parts)
    for current in [root, *[root.joinpath(*parts[:n]) for n in range(1, len(parts) + 1)]]:
        if current.is_symlink() or (hasattr(current, 'is_junction') and current.is_junction()):
            raise ValueError('Linked artifact path')
    if not path.resolve().is_relative_to(root.resolve()):
        raise ValueError('Artifact escaped owned workspace')
    return path


def identity(path, root=None):
    if root is not None:
        relative = path.relative_to(root).as_posix()
        path = safe_path(root, relative)
    else:
        relative = str(path)
    if not path.is_file() or path.stat().st_size > FILE_CAP:
        raise ValueError('File missing or exceeds cap')
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(65536), b''):
            digest.update(block)
    return {'path': relative, 'sha256': digest.hexdigest(), 'bytes': path.stat().st_size}


def read_bounded(path, cap):
    if not path.is_file() or path.stat().st_size>cap:
        raise ValueError('Structured stage input exceeds cap')
    with path.open('rb') as stream:
        raw=stream.read(cap+1)
    if len(raw)>cap:
        raise ValueError('Structured stage input changed beyond cap')
    return raw


def verify_identity(root, pin):
    if identity(safe_path(root, pin['path']), root) != pin:
        raise ValueError('Actual staged input changed: ' + pin['path'])


def write_new(path, raw):
    if len(raw) > FILE_CAP:
        raise ValueError('Output cap exceeded')
    if path.is_symlink() or (hasattr(path, 'is_junction') and path.is_junction()):
        raise ValueError('Linked output')
    with path.open('xb') as stream:
        stream.write(raw)


def copy_allowlist(root, output, names):
    paths = [safe_path(root, name) for name in names]
    if len(set(names)) != len(names) or sum(identity(p)['bytes'] for p in paths) > TOTAL_CAP:
        raise ValueError('Artifact allowlist exceeds cap or repeats a name')
    output.mkdir()
    for name, source in zip(names, paths):
        destination = safe_path(output, name)
        destination.parent.mkdir(parents=True, exist_ok=True)
        write_new(destination, source.read_bytes())


def host_observations(env):
    fixed = {'GITHUB_REPOSITORY': REPOSITORY, 'GITHUB_REF': SOURCE_REF,
             'GITHUB_WORKFLOW_REF': WORKFLOW_REF, 'GITHUB_JOB': 'native-discovery',
             'RUNNER_ARCH': 'X64', 'RUNNER_OS': 'Windows',
             'RUNNER_ENVIRONMENT': 'github-hosted', 'GITHUB_ACTIONS': 'true'}
    if any(env.get(k) != v for k, v in fixed.items()):
        raise ValueError('Wrong fixed hosted source/workflow context')
    for key in ['GITHUB_WORKFLOW_SHA', 'GITHUB_SHA']:
        if not re.fullmatch('[0-9a-f]{40}', env.get(key, '')):
            raise ValueError('Missing actual source/workflow SHA')
    for key in ['GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT']:
        if not re.fullmatch('[1-9][0-9]{0,18}', env.get(key, '')):
            raise ValueError('Missing actual run/attempt')
    for key in ['RUNNER_NAME', 'ImageOS']:
        value = env.get(key, '')
        if not value or len(value.encode()) > 128 or value.strip() != value or any(ord(c) < 32 for c in value):
            raise ValueError('Missing actual runner image/name')
    return dict(schema_version=1, repository=REPOSITORY, source_ref=SOURCE_REF,
                workflow_ref=WORKFLOW_REF, workflow_sha=env['GITHUB_WORKFLOW_SHA'],
                source_sha=env['GITHUB_SHA'], run_id=int(env['GITHUB_RUN_ID']),
                attempt=int(env['GITHUB_RUN_ATTEMPT']), job_name='native-discovery',
                runner_name=env['RUNNER_NAME'], runner_image=env['ImageOS'],
                arch='X64', os='Windows', environment='github-hosted', actions=True)


def select_active_job(document, run, attempt, runner):
    jobs = document.get('jobs', [])
    if not isinstance(jobs, list) or len(jobs) > 100 or document.get('total_count') != len(jobs):
        raise ValueError('Incomplete bounded active job list')
    matching = [j for j in jobs if j.get('name') == 'native-discovery'
                and j.get('status') == 'in_progress' and j.get('run_id') == run
                and j.get('run_attempt') == attempt and j.get('runner_name') == runner]
    if len(matching) != 1 or type(matching[0].get('id')) is not int or matching[0]['id'] <= 0:
        raise ValueError('No unique active job for this exact run/attempt/runner')
    return matching[0]['id']


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError('Cross-request redirects are forbidden')


def active_job(observed):
    url = ('https://api.github.com/repos/' + REPOSITORY + '/actions/runs/'
           + str(observed['run_id']) + '/attempts/' + str(observed['attempt']) + '/jobs?per_page=100')
    request = urllib.request.Request(url, headers={'Accept': 'application/vnd.github+json',
                                                  'User-Agent': 'FocusFlow-private-audio-source-stage'})
    # Public fixed-repository read only: no credential enters staging or audio descendants.
    deadline = time.monotonic() + 10
    with urllib.request.build_opener(NoRedirect()).open(request, timeout=10) as response:
        if response.geturl() != url or response.status != 200:
            raise ValueError('Unexpected job observation response')
        raw = bytearray()
        while True:
            if time.monotonic() >= deadline:
                raise ValueError('Job observation deadline exceeded')
            if response.isclosed():
                break
            response.fp.raw._sock.settimeout(max(0.001, deadline-time.monotonic()))
            block = response.read1(16384)
            if not block:
                if response.length:
                    raise ValueError('Incomplete job observation response')
                break
            raw.extend(block)
            if len(raw) > 1024 * 1024:
                raise ValueError('Job observation cap exceeded')
    return select_active_job(json.loads(raw), observed['run_id'], observed['attempt'], observed['runner_name'])


def run_logged(directory, label, argv, env, seconds, cap=8 * 1024 * 1024, stdin=None, *, retain_owner=False):
    """Bounded commands; only the controller retains ownership after active-work failure."""
    if stdin is not None and len(stdin) > 4096:
        raise ValueError('Controller input cap exceeded')
    started = time.monotonic()
    output = bytearray()
    overflow = threading.Event()
    log_failed = threading.Event()
    error = []
    if retain_owner:
        write_new(directory / (label + '.log'), b'')
    child = subprocess.Popen(argv, cwd=directory.parent, env=env, stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    exact_code = None
    def poll():
        nonlocal exact_code
        try:
            code = child.poll()
        except OSError:
            if not retain_owner:
                raise
            if not error:
                error.append('process-observation')
            return None
        if code is not None:
            exact_code = code
        return code
    def read():
        try:
            while True:
                block = (child.stdout.read1(4096) if retain_owner else child.stdout.read(4096))
                if not block:
                    return
                room = max(0, cap - len(output))
                output.extend(block[:room])
                if retain_owner and room and not log_failed.is_set():
                    try:
                        with (directory / (label + '.log')).open('ab') as live_log:
                            live_log.write(block[:room])
                    except OSError:
                        # A failed log sink must not block the controller's output/reaping path.
                        log_failed.set()
                if len(block) > room:
                    overflow.set()
                    if not retain_owner:
                        return
        except OSError as exc:
            error.append(type(exc).__name__)
    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    failure = None
    if stdin is not None:
        try:
            child.stdin.write(stdin)
            child.stdin.flush()
            if not retain_owner:
                child.stdin.close()
        except OSError:
            if not retain_owner:
                raise
            failure = 'pipe-write'
    failed_receipt = False
    while poll() is None or reader.is_alive():
        if failure is None:
            if log_failed.is_set():
                failure = 'log-write'
            elif overflow.is_set():
                failure = 'output-cap'
            elif error:
                failure = 'pipe-read'
            elif time.monotonic() - started >= seconds:
                failure = 'deadline'
        if failure:
            if not retain_owner:
                break
            if not failed_receipt:
                # EOF is deny-only cancellation; keep the exact controller until it finishes reaping.
                try:
                    child.stdin.close()
                except OSError:
                    pass
                pending = {'schema_version':1,'status':'Failed','cleanup_pending':True,
                           'exact_exit':False,'failure':failure,'active_seconds':seconds,
                           'collection':'Disabled','qualification':'Unqualified'}
                try:
                    write_new(directory / (label + '-failure.json'), encoded(pending))
                except (OSError, ValueError):
                    # Even a receipt-disk failure cannot shorten the exact owner's lifetime.
                    pass
                failed_receipt = True
        time.sleep(0.1 if failure and retain_owner else 0.01)
    if failure and not retain_owner and poll() is None:
        child.kill()
    if not retain_owner:
        try:
            child.wait(timeout=2)
        except subprocess.TimeoutExpired:
            failure = 'cleanup-pending'
    else:
        if log_failed.is_set():
            failure = failure or 'log-write'
        if error:
            failure = failure or 'pipe-read'
        if time.monotonic() - started >= seconds:
            failure = failure or 'deadline'
    if retain_owner:
        try:
            child.stdin.close()
        except OSError:
            pass
    reader.join(timeout=2)
    if reader.is_alive():
        failure = 'pipe-eof-pending'
    if overflow.is_set():
        failure = failure or 'output-cap'
    if not reader.is_alive():
        child.stdout.close()
    if not retain_owner:
        write_new(directory / (label + '.log'), bytes(output))
    tool=shutil.which(argv[0],path=env.get('PATH'))
    configuration={key:env[key] for key in ['CARGO_HOME','CARGO_TARGET_DIR','CARGO_BUILD_JOBS','RUSTC','RUSTDOC','CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER','LIB','RUSTFLAGS'] if key in env}
    receipt = {'argv': argv, 'tool':identity(Path(tool)) if tool else None,'configuration':configuration, 'exit_code': exact_code if retain_owner else poll(), 'exact_exit': exact_code is not None if retain_owner else poll() is not None,
               'failure': failure, 'elapsed_seconds': time.monotonic() - started,
               'output': identity(directory / (label + '.log'), directory)}
    write_new(directory / (label + '.json'), encoded(receipt))
    return receipt


def run_controller(directory, argv, env, stdin, seconds=640, cap=4 * 1024 * 1024):
    if not stdin.endswith(b'\n') or b'\n' in stdin[:-1]:
        raise ValueError('Controller needs one bounded request frame')
    return run_logged(directory, 'discovery-controller', argv, env, seconds, cap, stdin, retain_owner=True)


def checked(directory, label, argv, env, seconds=600):
    receipt = run_logged(directory, label, argv, env, seconds)
    if receipt['failure'] or receipt['exit_code'] != 0 or not receipt['exact_exit']:
        raise ValueError('Command incomplete: ' + label)
    return (directory / (label + '.log')).read_text(encoding='utf-8').strip()


def dep_words(text):
    # Rust dep-info escapes spaces/backslashes and uses the first ': ' as target delimiter.
    first = text.replace('\\\n', '').splitlines()[0]
    if ': ' not in first:
        raise ValueError('Malformed compiler dep-info')
    source = first.split(': ', 1)[1]
    words = re.findall(r'(?:\\.|[^\s])+', source)
    return [re.sub(r'\\([ \\#])', r'\1', word) for word in words]


def compiler_inputs(root, deps, manifest=None):
    paths = set()
    for dep in deps:
        for name in dep_words(dep.read_text(encoding='utf-8')):
            path = Path(name)
            if not path.is_absolute():
                path = (manifest or root) / path
            if any(p.is_symlink() or (hasattr(p,'is_junction') and p.is_junction()) for p in [path,*path.parents]):
                raise ValueError('Linked compiler input')
            path=path.resolve()
            try:
                relative = path.relative_to(root).as_posix()
            except ValueError as exc:
                raise ValueError('Compiler input lies outside frozen workspace') from exc
            paths.add(safe_path(root, relative))
    if not paths or len(paths) > 510:
        raise ValueError('Compiler source closure missing or exceeds pin budget')
    return [identity(p, root) for p in sorted(paths)]


def source_inputs(root):
    files = {root / WORKER / 'Cargo.toml', root / WORKER / 'Cargo.lock',
             root / WORKER / 'FIXTURE-SOURCE-MAP.json', root / WORKER / 'NATIVE-PROOF-PLAN.json',
             root / 'FocusFlow/crates/focusflow-core/Cargo.toml', root / 'FocusFlow/crates/focusflow-core/Cargo.lock',
             root / '.github/workflows/windows-audio-native-qualification.yml'}
    for folder, pattern in [(WORKER + '/src', '*.rs'), (WORKER + '/tests', '*.rs'),
                            ('FocusFlow/crates/focusflow-core/src', '*.rs'), ('tools/audio-proof', '*.py'),
                            ('tools/audio-proof', '*.ps1')]:
        files.update((root / folder).rglob(pattern))
    return [identity(p, root) for p in sorted(files)]


def safe_environment():
    # These are observations/runtime paths, never root-approved expected values or credentials.
    names = ['PATH', 'SystemRoot', 'WINDIR', 'TEMP', 'TMP', 'USERPROFILE', 'LOCALAPPDATA',
             'APPDATA', 'HOMEDRIVE', 'HOMEPATH', 'RUSTUP_HOME']
    return {name: os.environ[name] for name in names if name in os.environ}


def windows_directory_status(text):
    if not text or not text.strip():
        return 'empty'
    lines = text.splitlines()
    if len(lines) != 1:
        return 'multiple-lines'
    value = lines[0].strip()
    if (len(value) > 32767 or not re.match(r'^[A-Za-z]:[\\/]', value)
            or re.search(r'[\x00-\x1f<>:"|?*]', value[2:])
            or any(part in ('.', '..') for part in re.split(r'[\\/]', value[3:]))
            or not PureWindowsPath(value).is_absolute()):
        return 'not-local-absolute'
    path = Path(value)
    try:
        if any(p.is_symlink() or (hasattr(p, 'is_junction') and p.is_junction())
               for p in [path, *path.parents]):
            return 'linked-directory'
        return 'valid' if path.is_dir() else 'missing-directory'
    except (OSError, ValueError):
        return 'unavailable-directory'


def select_visual_studio(directory, vswhere, env):
    argv = [str(vswhere), '-latest', '-products', '*', '-requires',
            'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath']
    tool = identity(vswhere)

    def query(label, additions):
        if identity(vswhere) != tool:
            raise ValueError('Visual Studio discovery tool identity changed')
        receipt = run_logged(directory, label, argv[:], dict(env, **additions), 30, 65536)
        if identity(vswhere) != tool or receipt['tool'] != tool or receipt['argv'] != argv:
            raise ValueError('Visual Studio discovery tool identity or argv changed')
        if receipt['failure'] or receipt['exit_code'] != 0 or not receipt['exact_exit']:
            raise ValueError('Command incomplete: ' + label)
        raw = read_bounded(directory / (label + '.log'), 65536)
        try:
            text = raw.decode('utf-8')
        except UnicodeDecodeError:
            return '', 'invalid-encoding'
        return text, windows_directory_status(text)

    selected, status = query('vswhere', {})
    if status == 'valid':
        return Path(selected.strip())
    summary = {'schema_version': 1, 'status': 'aborted', 'candidates': {},
               'runs': {'vswhere': {'selection': status}}}
    candidates = {}
    try:
        for name in ['ProgramData', 'ALLUSERSPROFILE']:
            value = os.environ.get(name)
            validation = windows_directory_status(value) if value is not None else 'missing'
            if validation == 'valid' and value != value.strip():
                validation = 'nonliteral-directory'
            summary['candidates'][name] = {'present': value is not None, 'validation': validation}
            if validation == 'valid':
                candidates[name] = value
        for label, names in VS_COMPARISONS:
            if any(name not in candidates for name in names):
                summary['runs'][label] = {'selection': 'skipped'}
                continue
            summary['runs'][label] = {'selection': 'not-completed'}
            _, status = query(label, {name: candidates[name] for name in names})
            summary['runs'][label] = {'selection': status}
        summary['status'] = 'diagnostic-only'
    finally:
        raw = encoded(summary)
        if len(raw) > 8192:
            raise ValueError('Visual Studio selection diagnostic summary exceeds cap')
        write_new(directory / 'vswhere-summary.json', raw)
    raise ValueError('Visual Studio selection diagnostic: no usable baseline installation; '
                     'inspect vswhere.log, vswhere.json and vswhere-summary.json; build stopped')


def select_windows_sdk(lib):
    versions = [p for p in lib.iterdir()
                if p.is_dir() and re.fullmatch(r'[0-9]+(?:\.[0-9]+){3}', p.name)]
    if not versions:
        raise ValueError('No eligible Windows SDK version directories in ' + str(lib))
    return max(versions, key=lambda p: tuple(map(int,p.name.split('.'))))


def build(root, observed):
    directory = root / STAGE
    directory.mkdir()
    env = safe_environment()
    program_data = os.environ.get('ProgramData')
    if (program_data is None or windows_directory_status(program_data) != 'valid'
            or program_data != program_data.strip()):
        raise ValueError('Build requires a valid observed literal ProgramData directory')
    env['ProgramData'] = program_data
    env.update(CARGO_HOME=str(root / CARGO), CARGO_TARGET_DIR=str(root / TARGET),
               CARGO_BUILD_JOBS='2', CARGO_TERM_COLOR='never')
    observed['job_id'] = active_job(observed)
    head = checked(directory, 'git-head', ['git', 'rev-parse', 'HEAD'], env, 10)
    if head != observed['source_sha']:
        raise ValueError('Checkout differs from observed source SHA')
    before = source_inputs(root)
    checked(directory, 'rustup-install', ['rustup', 'toolchain', 'install', '1.89.0', '--profile', 'minimal', '--no-self-update'], env)
    sysroot = Path(checked(directory, 'sysroot', ['rustc', '+1.89.0', '--print', 'sysroot'], env, 30))
    rustc = sysroot / 'bin/rustc.exe'
    cargo = sysroot / 'bin/cargo.exe'
    checked(directory, 'rustc-version', [str(rustc), '-vV'], env, 30)
    checked(directory, 'cargo-version', [str(cargo), '--version'], env, 30)
    vswhere = Path('C:/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe')
    vs = select_visual_studio(directory, vswhere, env)
    versions = sorted((vs / 'VC/Tools/MSVC').iterdir(), key=lambda p: tuple(map(int,p.name.split('.'))))
    msvc = versions[-1]
    linker = msvc / 'bin/Hostx64/x64/link.exe'
    sdk = Path('C:/Program Files (x86)/Windows Kits/10')
    sdk_version = select_windows_sdk(sdk / 'Lib')
    lib_dirs = [msvc / 'lib/x64', sdk_version / 'um/x64', sdk_version / 'ucrt/x64']
    if any(not p.is_dir() for p in lib_dirs):
        raise ValueError('Selected linker/SDK libraries unavailable')
    env.update(RUSTC=str(rustc), RUSTDOC=str(sysroot / 'bin/rustdoc.exe'),
               CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=str(linker),
               LIB=';'.join(map(str,lib_dirs)), RUSTFLAGS='-C link-arg=/VERBOSE:LIB')
    checked(directory, 'cargo-build', [str(cargo), 'build', '--locked', '--jobs', '2', '--target',
            'x86_64-pc-windows-msvc', '--bin', 'private_audio_proof', '--manifest-path',
            str(root / WORKER / 'Cargo.toml'), '--verbose'], env, 1200)
    after = source_inputs(root)
    if before != after:
        raise ValueError('Reviewed source changed while compiling')
    deps = list((root / TARGET).rglob('*.d'))
    compiler = compiler_inputs(root, deps, root / WORKER)
    union = {p['path']: p for p in before + compiler}
    if len(union) > 510 or sum(p['bytes'] for p in union.values()) > TOTAL_CAP:
        raise ValueError('Runtime closure exceeds manifest pin budget')
    write_new(directory / 'compiler-input-map.json', encoded({'schema_version':1,'inputs':compiler}))
    write_new(directory / 'source-input-map.json', encoded({'schema_version':1,'inputs':list(union.values()),
              'review_only':sorted(set(p['path'] for p in before)-set(p['path'] for p in compiler))}))
    # Record the actual selected tool/SDK bytes, not a claimed version string alone.
    tool_paths=[rustc,cargo,sysroot/'bin/rustdoc.exe',linker,vswhere,Path(sys.executable)]
    tool_paths+=sorted((sysroot/'bin').glob('*.dll'))
    tool_paths+=sorted((sysroot/'lib/rustlib/x86_64-pc-windows-msvc/lib').glob('*.rlib'))
    tools=[identity(p) for p in tool_paths]
    sdk_libraries = [identity(p) for folder in lib_dirs for p in sorted(folder.glob('*.lib'))]
    write_new(directory / 'tool-identities.json', encoded({'schema_version':1,'tools':tools,
              'selected_sdk':sdk_version.name,'library_search_paths':list(map(str,lib_dirs)),
              'selected_sdk_library_inventory':sdk_libraries,'link_selection_log':'cargo-build.log'}))
    exe = root / TARGET / 'x86_64-pc-windows-msvc/debug/private_audio_proof.exe'
    write_new(directory / 'private_audio_proof.exe', exe.read_bytes())
    with zipfile.ZipFile(directory / 'source-files.zip', 'x', compression=zipfile.ZIP_DEFLATED) as archive:
        for pin in union.values():
            verify_identity(root,pin)
            archive.write(safe_path(root,pin['path']),pin['path'])
    observed['request_id'] = uuid.uuid4().hex
    observed['output_namespace'] = 'audio-discovery-{}-{}-{}'.format(observed['run_id'],observed['attempt'],observed['request_id'])
    request = {'schema_version':1,'kind':'BuildRequestV1','observations':observed,
               'executable':identity(exe,root),
               'source_input_map':identity(directory/'source-input-map.json',root),
               'compiler_input_map':identity(directory/'compiler-input-map.json',root),
               'tool_identities':identity(directory/'tool-identities.json',root),
               'collection':'Disabled','qualification':'Unqualified',
               'receipts':[identity(directory/(name+'.json'),root) for name in COMMANDS]}
    raw = encoded(request)
    write_new(directory/'build-request.json',raw)
    approval_request = dict(observed,request_sha256=hashlib.sha256(raw).hexdigest())
    if len(encoded(approval_request)) > 4096:
        raise ValueError('Approval observation request cap exceeded')
    write_new(directory/'approval-request.json',encoded(approval_request))
    names = BUILD_FILES[:-1] + [name+suffix for name in COMMANDS for suffix in ['.log','.json']]
    write_new(directory/'build-artifacts.json',encoded([identity(directory/name,directory) for name in names]))



def controller_workspace(path):
    # Rust canonicalize returns verbatim disk paths; preserve its existing strict admission gate.
    path=str(path)
    if not re.fullmatch(r'[A-Za-z]:\\[^\x00]*',path):
        raise ValueError('Expected canonical hosted local-drive workspace')
    return '\\\\?\\' + path


def discover(root, observed):
    directory = root / STAGE
    request_raw = read_bounded(directory/'approval-request.json',4096)
    if len(request_raw)>4096:
        raise ValueError('Approval request cap exceeded')
    request = json.loads(request_raw)
    build_raw = read_bounded(directory/'build-request.json',1024*1024)
    build_request = json.loads(build_raw)
    if request['request_sha256'] != hashlib.sha256(build_raw).hexdigest():
        raise ValueError('Build request changed before approval wait')
    for key,value in observed.items():
        if request.get(key)!=value:
            raise ValueError('Same active job context changed before approval wait')
    if active_job(observed)!=request['job_id']:
        raise ValueError('Original active job no longer matches')
    for name in ['executable','source_input_map','compiler_input_map','tool_identities']:
        verify_identity(root,build_request[name])
    for pin in json.loads(read_bounded(directory/'source-input-map.json',256*1024))['inputs']:
        verify_identity(root,pin)
    env=safe_environment()
    for name in ['GITHUB_REPOSITORY','GITHUB_REF','GITHUB_WORKFLOW_REF','GITHUB_WORKFLOW_SHA',
                 'GITHUB_SHA','GITHUB_RUN_ID','GITHUB_RUN_ATTEMPT','GITHUB_JOB','RUNNER_NAME',
                 'ImageOS','RUNNER_ARCH','RUNNER_OS','RUNNER_ENVIRONMENT','GITHUB_ACTIONS']:
        env[name]=os.environ[name]
    env.update(GITHUB_WORKSPACE=controller_workspace(root),FOCUSFLOW_AUDIO_JOB_ID=str(request['job_id']))
    exe=safe_path(root,build_request['executable']['path'])
    receipt=run_controller(directory,[str(exe),'--private-audio-controller'],env,request_raw)
    write_new(directory/'discovery-status.json',encoded({'schema_version':1,'controller':receipt,
        'qualification':'Unqualified','collection':'Disabled','route_ownership':'Root review required'}))

    if receipt['failure'] or receipt['exit_code']!=0:
        raise ValueError('Native discovery unavailable or failed; inspect exact controller receipt')


def package(root, phase):
    directory=root/STAGE
    names=(BUILD_FILES+[name+suffix for name in COMMANDS for suffix in ['.log','.json']]
           if phase=='build' else ['discovery-controller.log','discovery-controller.json','discovery-controller-failure.json','discovery-status.json'])
    if phase == 'build' and not (directory / 'build-artifacts.json').exists():
        names += VS_FAILURE_FILES
    available=[name for name in names if (directory/name).exists()]
    if not available:
        raise ValueError('No bounded stage receipts available')
    copy_allowlist(directory,directory/(phase+'-upload'),available)


def arguments(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase',choices=['build','discover','package-build','package-discovery'])
    return parser.parse_args(argv)


def main():
    args=arguments()
    if sys.platform!='win32':
        raise ValueError('Native staging requires approved hosted Windows source run')
    observed=host_observations(os.environ)
    root=Path(os.environ['GITHUB_WORKSPACE'])
    if root.resolve()!=root or not root.is_dir():
        raise ValueError('Hosted workspace must be canonical')
    if args.phase.startswith('package-'):
        package(root,args.phase.removeprefix('package-'))
    else:
        (build if args.phase=='build' else discover)(root,observed)


if __name__=='__main__':
    try:
        main()
    except (OSError,ValueError,KeyError,subprocess.SubprocessError) as exc:
        print('Private audio stage failed: '+str(exc),file=sys.stderr)
        raise SystemExit(1)
