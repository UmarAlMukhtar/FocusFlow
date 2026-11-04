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
