"""Literal close-subset routing and actual upload-glob coverage; no native/codec execution."""
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import run
from package_evidence import package

class Diagnostic(unittest.TestCase):
    def test_close_subset_builds_only_harness_and_never_reports_full_pass(self):
        for fail in [False,True]:
            with self.subTest(fail=fail),tempfile.TemporaryDirectory() as tmp:
                repo=Path(__file__).resolve().parents[2];root=Path(tmp)
                args=SimpleNamespace(evidence=root/'evidence',ffmpeg=root/'ffmpeg',ffprobe=root/'ffprobe',scope='close-diagnostic')
                metadata={'packages':[{'name':'windows-capture','source':None,'manifest_path':str(repo/'FocusFlow/vendor/windows-capture/Cargo.toml')}]}
                builds=[];calls=[]
                def build(repo,evidence,manifest,name,test):
                    builds.append((name,test));return root/'harness.exe'
                def execute(argv,evidence,name,*unused,**kwargs):
                    calls.append((name,argv));self.assertEqual(name,'close')
                    self.assertEqual(argv[2:6],['close',641,359,202])
                    case=evidence/'close/session-0';case.mkdir(parents=True)
                    if fail:raise RuntimeError('retained identity failure')
                with patch('run.sys.platform','win32'),patch.dict(os.environ,{'GITHUB_REF':'refs/heads/codex-windows-owned-close-diagnostic'}),patch('run.os.chdir'),patch('run.source_manifest',return_value={}),patch('run.sha',side_effect=lambda p:run.FFMPEG_SHA if p==args.ffmpeg else run.FFPROBE_SHA if p==args.ffprobe else 'fixture-hash'),patch('run.subprocess.check_output',side_effect=lambda argv,**kw:json.dumps(metadata) if argv[0]=='cargo' else 'fixture-head'),patch('run.build_exe',side_effect=build),patch('run.execute',side_effect=execute),patch('run.analyze.analyze'):
                    if fail:
                        with self.assertRaises(RuntimeError):run.run(args)
                    else:run.run(args)
                self.assertEqual(builds,[('windows_owned_qualification',True)]);self.assertEqual(len(calls),1)
                result=json.loads((args.evidence/'result.json').read_text())
                self.assertFalse(result['passed']);self.assertTrue(result['diagnosticOnly']);self.assertEqual(result['scope'],'close-diagnostic');self.assertEqual(result['diagnosticPassed'],not fail)
                provenance=json.loads((args.evidence/'provenance.json').read_text());self.assertEqual(provenance['scope'],'close-diagnostic');self.assertEqual(provenance['expectedBranch'],'codex-windows-owned-close-diagnostic')

    def test_default_full_scope_still_enters_required_publication_gate(self):
        repo=Path(__file__).resolve().parents[2]
        metadata={'packages':[{'name':'windows-capture','source':None,'manifest_path':str(repo/'FocusFlow/vendor/windows-capture/Cargo.toml')}]}
        class FirstGate(BaseException):pass
        def execute(argv,evidence,name,*args,**kw):
            self.assertEqual(name,'core-publication');self.assertIn('capture::windows::tests::',argv);raise FirstGate
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);args=SimpleNamespace(evidence=root/'e',ffmpeg=root/'f',ffprobe=root/'p')
            with patch('run.sys.platform','win32'),patch.dict(os.environ,{'GITHUB_REF':'refs/heads/codex-windows-owned-product-qualification'}),patch('run.os.chdir'),patch('run.source_manifest',return_value={}),patch('run.sha',side_effect=lambda p:run.FFMPEG_SHA if p==args.ffmpeg else run.FFPROBE_SHA),patch('run.subprocess.check_output',side_effect=lambda argv,**kw:json.dumps(metadata) if argv[0]=='cargo' else 'fixture-head'),patch('run.execute',side_effect=execute):
                with self.assertRaises(FirstGate):run.run(args)
            provenance=json.loads((args.evidence/'provenance.json').read_text());self.assertEqual(provenance['scope'],'full');self.assertFalse(provenance['diagnosticOnly'])

    def test_scope_ref_mismatch_is_rejected_before_tools(self):
        with tempfile.TemporaryDirectory() as tmp:
            a=SimpleNamespace(evidence=Path(tmp)/'evidence',ffmpeg=Path(tmp)/'f',ffprobe=Path(tmp)/'p',scope='close-diagnostic')
            with patch('run.sys.platform','win32'),patch.dict(os.environ,{'GITHUB_REF':'refs/heads/codex-windows-owned-product-qualification'}),patch('run.os.chdir'),patch('run.subprocess.check_output',side_effect=AssertionError('tool must not run')):
                with self.assertRaisesRegex(AssertionError,'scope/ref'):run.run(a)
            self.assertFalse(a.evidence.exists())

    def test_packaged_source_log_and_actions_match_real_workflow_upload_globs(self):
        workflow=(Path(__file__).resolve().parents[2]/'.github/workflows/windows-owned-product-qualification.yml').read_text()
        globs=[line.strip() for line in workflow.splitlines() if line.strip().startswith('qualification-upload/')]
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);ev=root/'evidence';case=ev/'close/session-0';case.mkdir(parents=True)
            (case/'source.log').write_bytes(b'');(case/'source-actions.json').write_text('{"records":[]}');(case/'private.log').write_text('never upload')
            hashes=package(ev,root/'absent',root/'qualification-upload')
            expected={'cases/close/session-0/source.log','cases/close/session-0/source-actions.json'}
            self.assertEqual(set(hashes),expected)
            uploaded={p.relative_to(root/'qualification-upload').as_posix() for glob in globs for p in root.glob(glob) if p.is_file()}
            self.assertTrue(expected<=uploaded);self.assertFalse(any('private' in p for p in uploaded))
            (case/'source-actions.json').write_bytes(b' '*4097)
            with self.assertRaises(AssertionError):run.bound(ev)
            with self.assertRaises(RuntimeError):package(ev,root/'absent',root/'capped')

if __name__=='__main__':unittest.main()
