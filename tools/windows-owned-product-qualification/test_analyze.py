import copy
import unittest
import subprocess
import tempfile
from pathlib import Path
import shutil
import json
import analyze

class Contract(unittest.TestCase):
    def trace(self):
        # Same QPC later publication must not rewrite frame zero. Equal native latest wins frame one.
        return dict(frequency=3000000, admission=3000000, epoch=10000000, cutoff=3200000, selected=2, written=2,
                    width=160,height=120,overflow=False, events=[
            dict(kind='source',contentWidth=160,contentHeight=120,textureWidth=160,textureHeight=120,order=0,seq=0,native=9999999,qpc=3000000,paint=9,nonce=7,corners=[40,100,160,220]),
            dict(kind='source',contentWidth=160,contentHeight=120,textureWidth=160,textureHeight=120,order=1,seq=1,native=10000000,qpc=3000001,paint=10,nonce=7,corners=[40,100,160,220]),
            dict(kind='decision',order=2,ordinal=0,seq=1,qpc=3100000),
            dict(kind='source',contentWidth=160,contentHeight=120,textureWidth=160,textureHeight=120,order=3,seq=2,native=10000000,qpc=3100000,paint=11,nonce=7,corners=[40,100,160,220]),
            dict(kind='decision',order=4,ordinal=1,seq=2,qpc=3200000)],
            writes=[dict(ordinal=0,seq=1,bytes=76800,ok=True),dict(ordinal=1,seq=2,bytes=76800,ok=True)])
    def test_equal_time_frontier(self):
        self.assertEqual([p['paint'] for p in analyze.expected(self.trace())],[10,11])
    def test_negative_causal_controls(self):
        for mutate in [lambda t:t['events'][2].update(seq=2),lambda t:t.update(cutoff=3199999),lambda t:t['writes'][1].update(ok=False),lambda t:t.update(overflow=True),lambda t:t.update(failure='encoder exit7'),lambda t:t['events'][1].update(contentWidth=159),lambda t:t['events'][3].update(native=9999998),lambda t:t['events'][2].update(qpc=3099999),lambda t:t['events'][3].update(order=2)]:
            t=self.trace();mutate(t)
            with self.assertRaises(AssertionError):analyze.expected(t)
    def image(self,paint=37,nonce=173,w=160,h=120):
        buf=bytearray([96]*(w*h*3))
        def rect(x,y,ww,hh,c):
            for yy in range(y,y+hh):
                for xx in range(x,x+ww):buf[(yy*w+xx)*3:(yy*w+xx)*3+3]=bytes([c]*3)
        for x,y,c in [(0,0,40),(w-16,0,100),(0,h-16,160),(w-16,h-16,220)]:rect(x,y,16,16,c)
        for yy,v in [(40,paint),(56,nonce),(72,paint),(88,nonce)]:
            for b in range(16):rect(16+b*8,yy,8,8,240 if v&(1<<b) else 12)
        return buf
    def test_lossy_tolerance_and_redundancy(self):
        image=self.image();self.assertEqual(analyze.identity(image,160,120,160,120),(37,173,[40,100,160,220]))
        # A bounded luma error survives; wrong paint, nonce, corners, frame order and geometry do not.
        noisy=bytes(min(255,c+9) for c in image)
        analyze.check_identity(noisy,160,120,dict(paint=37,nonce=173,corners=[40,100,160,220]),160,120)
        for p,n in [(38,173),(37,174)]:
            with self.assertRaises(AssertionError):analyze.check_identity(self.image(p,n),160,120,dict(paint=37,nonce=173,corners=[40,100,160,220]),160,120)
        image[8*160*3+8*3:8*160*3+8*3+3]=b'\xff'*3
        with self.assertRaises(AssertionError):analyze.identity(image,160,120,160,120)
        with self.assertRaises(AssertionError):analyze.identity(self.image(),162,120,160,120)
    def test_real_product_codec_marker_thresholds(self):
        ffmpeg=shutil.which('ffmpeg');ffprobe=shutil.which('ffprobe')
        self.assertIsNotNone(ffmpeg);self.assertIsNotNone(ffprobe)
        with tempfile.TemporaryDirectory() as temp:
            for w,h in [(320,240),(641,359),(803,451),(960,540),(161,479)]:
                ew,eh=w+w%2,h+h%2
                frames=[]
                for paint in [1,2,255,256,65534,65535]:
                    rgb=self.image(paint,173,w,h);bgra=bytearray(ew*eh*4)
                    for y in range(h):
                        for x in range(w):
                            at=(y*w+x)*3;to=(y*ew+x)*4
                            bgra[to:to+4]=bytes([rgb[at+2],rgb[at+1],rgb[at],255])
                    frames.append(bytes(bgra))
                out=Path(temp)/f'{w}x{h}.mp4'
                subprocess.run([ffmpeg,'-hide_banner','-loglevel','error','-nostdin','-n','-f','rawvideo','-pixel_format','bgra','-video_size',f'{ew}x{eh}','-framerate','30','-i','pipe:0','-map','0:v:0','-an','-c:v','libx264','-preset','veryfast','-crf','23','-pix_fmt','yuv420p','-bf','0','-fps_mode','passthrough','-video_track_timescale','30000','-movflags','+faststart','-f','mp4',str(out)],input=b''.join(frames),check=True,timeout=20)
                meta=json.loads(subprocess.check_output([ffprobe,'-v','error','-show_streams','-show_packets','-of','json',str(out)],timeout=10))
                analyze.packets(meta['packets'],meta['streams'][0]['time_base'],6)
                decoded=subprocess.check_output([ffmpeg,'-v','error','-i',str(out),'-f','rawvideo','-pix_fmt','rgb24','pipe:1'],timeout=20)
                self.assertEqual(len(decoded),ew*eh*3*6)
                for n,paint in enumerate([1,2,255,256,65534,65535]):
                    analyze.check_identity(decoded[n*ew*eh*3:(n+1)*ew*eh*3],ew,eh,dict(paint=paint,nonce=173,corners=[40,100,160,220]),w,h)
    def test_packet_grid_negative_controls(self):
        packets=[dict(pts=str(n*1000),dts=str(n*1000),duration='1000') for n in range(2)]
        analyze.packets(packets,'1/30000',2)
        for key in ['pts','dts','duration']:
            bad=copy.deepcopy(packets);bad[1][key]='999'
            with self.assertRaises(AssertionError):analyze.packets(bad,'1/30000',2)
        with self.assertRaises(AssertionError):analyze.packets(list(reversed(packets)),'1/30000',2)
if __name__=='__main__':unittest.main()
