import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from analyze import analyze, decode, summarize


class AnalysisTests(unittest.TestCase):
    def test_decodes_literal_marker_and_all_four_color_corners(self):
        # A literal ID 165 and asymmetric color corners catch bit-order/channel/origin mistakes.
        rgb = bytearray([96] * (640 * 360 * 3))
        for x, y, color in [(0, 0, (255, 0, 0)), (608, 0, (0, 255, 0)),
                            (0, 328, (0, 0, 255)), (608, 328, (255, 255, 0))]:
            for row in range(y, y + 32):
                for col in range(x, x + 32):
                    rgb[(row * 640 + col) * 3:(row * 640 + col) * 3 + 3] = bytes(color)
        for bit in range(16):
            color = 240 if bit in (0, 2, 5, 7) else 12
            for row in range(128, 192):
                for col in range(32 + bit * 32, 56 + bit * 32):
                    rgb[(row * 640 + col) * 3:(row * 640 + col) * 3 + 3] = bytes([color] * 3)
        self.assertEqual(decode(rgb)['id'], 165)
        self.assertTrue(decode(rgb)['cornersValid'])
        rgb[0:32*640*3] = bytes([96]) * (32*640*3)
        self.assertFalse(decode(rgb)['cornersValid'])
        self.assertRaises(ValueError, decode, rgb[:100])

    def fixture(self):
        events = [{'event': 'startup', 'frequency': 10_000_000, 'qpc': 900_000,
                   'cursorRequest': 'WithoutCursor'}, {'event': 'captureStarted'}]
        for seq, native in enumerate([1_000_000, 1_333_333, 1_666_666]):
            events.append(dict(event='enqueued', seq=seq, native100ns=native,
                               first100ns=1_000_000, delta100ns=native-1_000_000,
                               width=640, height=360, id=seq,
                               qpcBefore=native+10, qpcSubmitBefore=native+20,
                               qpcAfter=native+30, qpcBefore100ns=native+10,
                               qpcAfter100ns=native+30))
        events.append({'event': 'finishComplete', 'enqueued': 3})
        probe = dict(streams=[dict(codec_type='video', width=640, height=360,
                                  time_base='1/10000000')],
                     frames=[dict(pts=x, duration=333333) for x in [0, 333333, 666666]],
                     packets=[dict(pts=x, duration=333333) for x in [666666, 0, 333333]])
        pixels = [dict(id=i, cornersValid=True) for i in range(3)]
        return events, probe, pixels

    def test_full_saved_pts_and_ids_match_and_packet_decode_order_is_independent(self):
        result = summarize(*self.fixture(), 'animated')
        self.assertEqual(result['status'], 'observed-preservation')
        self.assertEqual(result['savedEndpointSeconds'], '999999/10000000')
        self.assertFalse(result['authorityAccepted'])

    def test_interior_resampling_is_visible_even_when_count_first_last_match(self):
        events, probe, pixels = self.fixture()
        probe['frames'][1]['pts'] = 400000
        probe['packets'][2]['pts'] = 400000
        result = summarize(events, probe, pixels, 'animated')
        self.assertEqual(result['status'], 'resampled-or-unresolved')
        self.assertEqual(result['ordinalResidual100ns'], ['0', '66667', '0'])

    def test_dropped_frame_and_marker_mismatch_do_not_pass_mapping(self):
        events, probe, pixels = self.fixture()
        pixels[1]['id'] = 77
        self.assertEqual(summarize(events, probe, pixels, 'animated')['status'],
                         'resampled-or-unresolved')
        events, probe, pixels = self.fixture()
        probe['frames'].pop(1); probe['packets'].pop(2); pixels.pop(1)
        self.assertEqual(summarize(events, probe, pixels, 'animated')['status'],
                         'resampled-or-unresolved')

    def test_missing_finish_duration_and_corrupt_markers_fail_closed(self):
        for defect in ('finish', 'duration', 'marker', 'regression', 'qpc'):
            events, probe, pixels = self.fixture()
            if defect == 'finish': events.pop()
            if defect == 'duration': del probe['frames'][-1]['duration']
            if defect == 'marker': pixels[1]['cornersValid'] = False
            if defect == 'regression': events[3]['native100ns'] = 999999
            if defect == 'qpc': events[3]['qpcBefore'] = events[3]['qpcAfter'] + 1
            with self.subTest(defect=defect), self.assertRaises(ValueError):
                summarize(events, probe, pixels, 'animated')

    def test_close_and_stall_require_the_real_scenario_event(self):
        for scenario in ('close', 'stall'):
            with self.subTest(scenario=scenario), self.assertRaises(ValueError):
                summarize(*self.fixture(), scenario)

    def test_static_ids_are_reported_as_ambiguous_even_when_pts_match(self):
        events, probe, pixels = self.fixture()
        for row in events:
            if row['event'] == 'enqueued': row['id'] = 0
        for row in pixels: row['id'] = 0
        result = summarize(events, probe, pixels, 'static')
        self.assertEqual(result['ambiguousIdFrames'], 3)
        self.assertFalse(result['authorityAccepted'])


class RealMediaTests(unittest.TestCase):
    @unittest.skipUnless(os.environ.get('PROBE_FFMPEG') and os.environ.get('PROBE_FFPROBE'),
                         'set exact FFmpeg/FFprobe paths for generated-media integration')
    def test_saved_h264_decoding_and_exact_endpoint_without_native_claim(self):
        # This is synthetic media, not a compositor/Windows result.
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as temporary:
            root = Path(temporary)
            raw = bytearray()
            for ident in (0, 1, 2):
                rgb = bytearray([96] * (640 * 360 * 3))
                for x, y, color in [(0, 0, (255, 0, 0)), (608, 0, (0, 255, 0)),
                                    (0, 328, (0, 0, 255)), (608, 328, (255, 255, 0))]:
                    for row in range(y, y + 32):
                        for col in range(x, x + 32):
                            rgb[(row*640+col)*3:(row*640+col)*3+3] = bytes(color)
                for bit in range(16):
                    c = 240 if ident & (1 << bit) else 12
                    for row in range(128, 192):
                        for col in range(32 + bit*32, 56 + bit*32):
                            rgb[(row*640+col)*3:(row*640+col)*3+3] = bytes([c]*3)
                raw.extend(rgb)
            subprocess.run([os.environ['PROBE_FFMPEG'], '-nostdin', '-v', 'error',
                            '-f', 'rawvideo', '-pixel_format', 'rgb24', '-video_size', '640x360',
                            '-framerate', '30', '-i', '-', '-an', '-c:v', 'libx264',
                            '-pix_fmt', 'yuv420p', str(root/'screen.mp4')],
                           input=raw, check=True, timeout=30)
            events, _, _ = AnalysisTests().fixture()
            import json
            (root/'native.jsonl').write_text(''.join(json.dumps(e)+'\n' for e in events))
            result = analyze(root, 'animated', os.environ['PROBE_FFPROBE'], os.environ['PROBE_FFMPEG'])
            self.assertEqual(result['status'], 'observed-preservation')
            self.assertEqual(result['savedCount'], 3)
            self.assertEqual(result['savedIds'], [0, 1, 2])
            self.assertEqual(result['savedEndpointSeconds'], '1/10')
            self.assertFalse((root/'decode-temporary.rgb').exists())
            self.assertFalse(result['authorityAccepted'])


if __name__ == '__main__':
    unittest.main()
