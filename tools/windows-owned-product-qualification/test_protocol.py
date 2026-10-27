import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import run


class Protocol(unittest.TestCase):
    def test_cargo_metadata_utf8_child_under_cp1252_default(self):
        repo=Path(__file__).resolve().parents[2]
        metadata={'packages':[{'name':'windows-capture','source':None,'manifest_path':str(repo/'FocusFlow/vendor/windows-capture/Cargo.toml'),'description':'\u0101'}]}
        payload=json.dumps(metadata,ensure_ascii=False).encode('utf-8')
        self.assertIn(b'\x81',payload)
        original=subprocess.check_output
        def child(argv,**kwargs):
            if argv[0]=='cargo':data=payload
            else:
                self.assertEqual(argv,['git','rev-parse','HEAD']);data=b'owned-fixture-sha\n'
            return original([sys.executable,'-c','import sys;sys.stdout.buffer.write('+repr(data)+')'],**kwargs)
        class PreflightComplete(BaseException):pass
        with tempfile.TemporaryDirectory() as tmp, patch('subprocess._text_encoding',return_value='cp1252'):
            with self.assertRaises(UnicodeDecodeError):
                original([sys.executable,'-c','import sys;sys.stdout.buffer.write('+repr(payload)+')'],text=True)
            args=SimpleNamespace(evidence=Path(tmp)/'evidence',ffmpeg=Path(tmp)/'ffmpeg',ffprobe=Path(tmp)/'ffprobe')
            with patch('run.sys.platform','win32'), patch('run.os.chdir'), patch('run.source_manifest',return_value={}), patch('run.sha',side_effect=lambda p:run.FFMPEG_SHA if p==args.ffmpeg else run.FFPROBE_SHA), patch('run.subprocess.check_output',side_effect=child), patch('run.execute',side_effect=PreflightComplete):
                with self.assertRaises(PreflightComplete):run.run(args)
            self.assertEqual(json.loads((args.evidence/'dependency-resolution.json').read_text(encoding='utf-8')),metadata)

    def test_utf8_artifact_and_fault_json_under_cp1252_default(self):
        original=Path.read_text
        def cp1252_default(path,encoding=None,errors=None,**kwargs):
            return original(path,encoding=encoding or 'cp1252',errors=errors,**kwargs)
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);exe=root/'\u0101.exe';log=root/'build.log'
            log.write_bytes((json.dumps({'reason':'compiler-artifact','target':{'name':'fixture'},'executable':str(exe)},ensure_ascii=False)+'\n').encode('utf-8'))
            trace=dict(encoderExitCode=7,nonEncoderFailure=False,failure='Encoder exited 7',supervisorTerminated=None,overflow=False,selected=1,written=1,width=320,height=240,writes=[dict(ok=True,bytes=320*240*4)],localeProbe='\u0101')
            (root/'trace.json').write_bytes(json.dumps(trace,ensure_ascii=False).encode('utf-8'))
            (root/'consumer-prefix.json').write_bytes(json.dumps(dict(eof=True,consumerBytes=320*240*4,notWriterProgress=True,localeProbe='\u0101'),ensure_ascii=False).encode('utf-8'))
            (root/'screen.mp4').write_bytes(b'fixture; decoder never reached')
            class TraceRead(BaseException):pass
            def check_trace(value):
                self.assertEqual(value,trace);raise TraceRead
            with patch.object(Path,'read_text',cp1252_default), patch('run.execute',return_value=log):
                self.assertEqual(run.build_exe(root,root,'unused','fixture'),exe.resolve())
                run.require_exit7(root)
                with patch('run.analyze.expected',side_effect=check_trace), self.assertRaises(TraceRead):
                    run.analyze.analyze(root,'unused','unused')
                log.write_bytes(b'{"bad":"\x81"}')
                with self.assertRaises(UnicodeDecodeError):run.build_exe(root,root,'unused','fixture')
                (root/'trace.json').write_bytes(b'{"bad":"\x81"}')
                with self.assertRaises(UnicodeDecodeError):run.require_exit7(root)
                with self.assertRaises(UnicodeDecodeError):run.analyze.analyze(root,'unused','unused')

    def test_checkout_override_preserves_lf_crlf_and_mixed_blob_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);env={**os.environ,'GIT_CONFIG_GLOBAL':str(root/'no-global'),'GIT_CONFIG_NOSYSTEM':'1'}
            def git(*argv,input=None,override=False):
                scoped={**env,**({'GIT_CONFIG_COUNT':'1','GIT_CONFIG_KEY_0':'core.autocrlf','GIT_CONFIG_VALUE_0':'false'} if override else {})}
                return subprocess.check_output(['git','-C',str(root),*argv],input=input,env=scoped,stderr=subprocess.STDOUT)
            git('init','--quiet');git('config','--local','core.autocrlf','true')
            blobs={'lf.txt':b'first\nsecond\n','crlf.txt':b'first\r\nsecond\r\n','mixed.txt':b'first\nsecond\r\n'}
            for name,data in blobs.items():
                oid=git('hash-object','-w','--stdin',input=data).decode('ascii').strip()
                # Only this owned temporary index is populated; no project commit or configuration changes.
                git('update-index','--add','--cacheinfo','100644',oid,name)
            git('checkout-index','--all','--force')
            self.assertEqual((root/'lf.txt').read_bytes(),blobs['lf.txt'].replace(b'\n',b'\r\n'))
            git('checkout-index','--all','--force',override=True)
            for name,data in blobs.items():self.assertEqual((root/name).read_bytes(),data)
            self.assertEqual(git('config','--local','--get','core.autocrlf'),b'true\n')


if __name__=='__main__':unittest.main()
