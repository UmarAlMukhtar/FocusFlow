"""Device-free preparation checks; these do not execute an encoder."""
import pathlib
import ast
import re
import importlib.util
import struct
import unittest

import validate


spec = importlib.util.spec_from_file_location('windows_audio_profile', validate.BASE / 'windows-audio-profile.py')
audio = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audio)


class AudioProfileControls(unittest.TestCase):
    def test_recipe_admits_required_float_io_and_audio_filters(self):
        try:
            admitted = validate.validate_audio_profile((validate.BASE / 'build.sh').read_text())
        except ValueError as error:
            self.fail(str(error))
        self.assertEqual(admitted,
                         {'filter': {'amix', 'asetrate', 'afade'}, 'encoder': {'pcm_f32le'},
                          'demuxer': {'pcm_f32le'}, 'muxer': {'pcm_f32le'}})

    def test_missing_capability_and_cli_name_in_configure_fail_closed(self):
        recipe = (validate.BASE / 'build.sh').read_text()
        for suffix, kind in [(',amix', 'filter'), (',asetrate', 'filter'), (',afade', 'filter')]:
            with self.subTest(capability=suffix), self.assertRaisesRegex(ValueError, kind):
                validate.validate_audio_profile(recipe.replace(suffix, ''))
        for kind in ['encoder', 'demuxer', 'muxer']:
            prefix = '--enable-' + kind + '='
            line = next(line for line in recipe.splitlines() if prefix in line)
            option = next(token for token in line.split() if token.startswith(prefix))
            for replacement in [option.replace(',pcm_f32le', ''), option.replace('pcm_f32le', 'f32le')]:
                with self.subTest(kind=kind, replacement=replacement), self.assertRaisesRegex(ValueError, kind):
                    validate.validate_audio_profile(recipe.replace(option, replacement))

    def test_inspector_requires_exact_extended_component_sets(self):
        # Run the real component gate without compiling or invoking its PE inspection.
        tree = ast.parse((validate.BASE / 'inspect-candidate.py').read_text())
        start = next(i for i, node in enumerate(tree.body) if isinstance(node, ast.Assign)
                     and any(isinstance(target, ast.Name) and target.id == 'kinds' for target in node.targets))
        gate = compile(ast.Module(body=tree.body[start:start + 3], type_ignores=[]), 'component-gate', 'exec')
        baseline = (validate.BASE / 'provenance/linux-config_components.h').read_text()
        windows = baseline.replace('#define CONFIG_XCBGRAB_INDEV 1', '#define CONFIG_XCBGRAB_INDEV 0')
        windows = windows.replace('#define CONFIG_GDIGRAB_INDEV 0', '#define CONFIG_GDIGRAB_INDEV 1')
        if '#define CONFIG_GDIGRAB_INDEV 1' not in windows:
            windows += '\n#define CONFIG_GDIGRAB_INDEV 1\n'
        extended = windows
        for component in ['AMIX_FILTER', 'ASETRATE_FILTER', 'AFADE_FILTER', 'PCM_F32LE_ENCODER',
                          'PCM_F32LE_DEMUXER', 'PCM_F32LE_MUXER']:
            if '#define CONFIG_' + component + ' 0' in extended:
                extended = extended.replace('#define CONFIG_' + component + ' 0', '#define CONFIG_' + component + ' 1')
            else:
                extended += '\n#define CONFIG_' + component + ' 1\n'
        context = {'re': re, 'linux': baseline, 'config': extended}
        exec(gate, context)
        self.assertEqual(context['sets']['INDEV'], ['GDIGRAB', 'LAVFI'])
        for wrong in [windows, extended + '\n#define CONFIG_UNREVIEWED_FILTER 1\n']:
            with self.assertRaises(AssertionError):
                exec(gate, {'re': re, 'linux': baseline, 'config': wrong})

    def test_mixing_check_rejects_gain_changes_channel_swaps_and_truncation(self):
        audio.check_pcm((.0625, -.125, .125, -.25), (.0625, -.125, .125, -.25))
        for actual in [(.125, -.25, .25, -.5), (-.125, .0625, -.25, .125),
                       (.0625, -.125), (float('nan'), -.125, .125, -.25)]:
            with self.subTest(actual=actual), self.assertRaises(ValueError):
                audio.check_pcm(actual, (.0625, -.125, .125, -.25))

    def test_rate_smoke_accepts_polyphase_ripple_and_rejects_signal_errors(self):
        # Execute the real rate assertion against the retained two-phase float32 values.
        tree = ast.parse((validate.BASE / 'windows-audio-profile.py').read_text())
        call = next(node for node in ast.walk(tree) if isinstance(node, ast.Call)
                    and isinstance(node.func, ast.Name) and node.func.id == 'check_pcm'
                    and isinstance(node.args[0], ast.Subscript)
                    and isinstance(node.args[0].value, ast.Name) and node.args[0].value.id == 'rated')
        assertion = compile(ast.Expression(call), 'rate-smoke-assertion', 'eval')
        rated = (.25, -.5, .24999728798866272, -.49999457597732544) * 1024
        try:
            eval(assertion, vars(audio), {'rated': rated})
        except ValueError as error:
            self.fail(str(error))
        wrong = [tuple(value * 1.001 for value in rated),
                 tuple(value for left, right in zip(rated[::2], rated[1::2]) for value in (right, left)),
                 rated[:-1], rated + (.25,)]
        for value in [float('nan'), float('inf'), -float('inf'), .25002, .24998]:
            wrong.append(rated[:256] + (value,) + rated[257:])
        for actual in wrong:
            with self.subTest(sample_count=len(actual), first_interior=actual[256]), self.assertRaises(ValueError):
                eval(assertion, vars(audio), {'rated': actual})

    def test_nonresampled_pcm_keeps_strict_amplitude_tolerance(self):
        # A 2e-6 drift must still fail each real roundtrip, mix and fade assertion.
        tree = ast.parse((validate.BASE / 'windows-audio-profile.py').read_text())
        calls = [node for node in ast.walk(tree) if isinstance(node, ast.Call)
                 and isinstance(node.func, ast.Name) and node.func.id == 'check_pcm'
                 and not isinstance(node.args[0], ast.Subscript)]
        self.assertEqual(len(calls), 3)
        for drift in [2e-6, -2e-6]:
            context = dict(vars(audio), first=None, inp=lambda _: [],
                           render=lambda *args: (.25 + drift, -.5) * 1024,
                           mixed=(.0625 + drift, -.125) * 512 + (.125, -.25) * 512,
                           faded=(.25 + drift, -.5) * 1024)
            for call in calls:
                with self.subTest(line=call.lineno, drift=drift), self.assertRaises(ValueError):
                    eval(compile(ast.Expression(call), 'strict-pcm-assertion', 'eval'), context)

    def test_pcm_float_frame_and_channel_bounds(self):
        self.assertEqual(audio.pcm_samples(struct.pack('<4f', .25, -.5, 0, 1), 2, 2),
                         (.25, -.5, 0, 1))
        for frames, channels, data in [(0, 2, b''), (96001, 2, b''), (1, 3, bytes(12)),
                                       (1, 2, bytes(4)), (1, 1, bytes(8)),
                                       (1, 1, struct.pack('<f', float('nan'))),
                                       (1, 1, struct.pack('<f', float('inf')))]:
            with self.subTest(frames=frames, channels=channels, data=data), self.assertRaises(ValueError):
                audio.pcm_samples(data, frames, channels)


if __name__ == '__main__':
    unittest.main()
