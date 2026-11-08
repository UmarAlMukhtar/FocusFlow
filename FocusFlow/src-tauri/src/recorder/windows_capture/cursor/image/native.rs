//! Win32 resource ownership extracted from the private qualification helper.
use crate::recorder::windows_capture::cursor::image::{self as math, Shape};
use windows::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::*};
type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub(in crate::recorder) struct Icon(pub(in crate::recorder) HICON);
impl Drop for Icon {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyIcon(self.0);
        }
    }
}
pub(in crate::recorder) struct Bitmaps(pub(in crate::recorder) ICONINFO);
impl Drop for Bitmaps {
    fn drop(&mut self) {
        unsafe {
            if !self.0.hbmColor.is_invalid() {
                let _ = DeleteObject(self.0.hbmColor.into());
            }
            if !self.0.hbmMask.is_invalid() {
                let _ = DeleteObject(self.0.hbmMask.into());
            }
        }
    }
}
struct ScreenDc(HDC);
impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(None, self.0);
        }
    }
}
pub(in crate::recorder) fn bitmap(bitmap: HBITMAP) -> TestResult<(u32, u32, Vec<u8>)> {
    let mut object = BITMAP::default();
    // GetIconInfo bitmaps are owned, unselected, and remain alive throughout GetObject/GetDIBits.
    unsafe {
        if GetObjectW(
            bitmap.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut object as *mut _ as _),
        ) != std::mem::size_of::<BITMAP>() as i32
        {
            return Err("GetObject bitmap read incomplete".into());
        }
    }
    let (w, h) = (
        u32::try_from(object.bmWidth)?,
        u32::try_from(object.bmHeight)?,
    );
    if w == 0 || h == 0 || w > 512 || h > 1024 {
        return Err("Cursor bitmap outside allocation cap".into());
    }
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let dc = ScreenDc(unsafe { GetDC(None) });
    if dc.0.is_invalid() {
        return Err("GetDC failed".into());
    }
    let mut pixels = vec![0; w as usize * h as usize * 4];
    // Top-down 32bpp rows have width*4 stride; a partial scan count is a failure, never a usable image.
    let lines = unsafe {
        GetDIBits(
            dc.0,
            bitmap,
            0,
            h,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    if lines != h as i32 {
        return Err("GetDIBits scan count incomplete".into());
    }
    Ok((w, h, pixels))
}
pub(in crate::recorder) struct Raster {
    pub(in crate::recorder) dc: HDC,
    pub(in crate::recorder) bitmap: HBITMAP,
    pub(in crate::recorder) previous: HGDIOBJ,
    pub(in crate::recorder) bits: *mut u8,
    pub(in crate::recorder) len: usize,
}
impl Drop for Raster {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}
impl Raster {
    pub(in crate::recorder) fn new(w: u32, h: u32) -> TestResult<Self> {
        if w == 0 || h == 0 || w > 512 || h > 512 {
            return Err("Raster allocation cap".into());
        }
        // DC and DIB are owned; each failure releases every successfully acquired predecessor.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return Err("Raster DC failed".into());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                Ok(b) => b,
                Err(e) => {
                    let _ = DeleteDC(dc);
                    return Err(e.into());
                }
            };
            let previous = SelectObject(dc, bitmap.into());
            if previous.is_invalid() || bits.is_null() {
                if !previous.is_invalid() {
                    SelectObject(dc, previous);
                }
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(dc);
                return Err("Raster selection failed".into());
            }
            Ok(Self {
                dc,
                bitmap,
                previous,
                bits: bits.cast(),
                len: w as usize * h as usize * 4,
            })
        }
    }
}
pub(in crate::recorder) fn decode_with(
    icon: &Icon,
    mut observe: impl FnMut(u8, Option<(u32, u32, &[u8])>) -> TestResult<()>,
) -> TestResult<Shape> {
    let mut info = Bitmaps(ICONINFO::default());
    let result = unsafe { GetIconInfo(icon.0, &mut info.0) };
    result?;
    observe(2, None)?;
    if info.0.fIcon.as_bool() || info.0.hbmMask.is_invalid() {
        return Err("Not a cursor or missing mask".into());
    }
    let hotspot = [info.0.xHotspot, info.0.yHotspot];
    let (mw, mh, mask) = bitmap(info.0.hbmMask)?;
    observe(3, None)?;
    let shape = if info.0.hbmColor.is_invalid() {
        if mh % 2 != 0 {
            return Err("Odd monochrome mask height".into());
        }
        let half = mh as usize / 2 * mw as usize;
        let bits: Vec<bool> = mask
            .chunks_exact(4)
            .map(|p| p[0] != 0 || p[1] != 0 || p[2] != 0)
            .collect();
        math::monochrome(mw, mh / 2, hotspot, &bits[..half], &bits[half..])?
    } else {
        let (w, h, bgra) = bitmap(info.0.hbmColor)?;
        if (w, h) != (mw, mh) {
            return Err("Mask/color dimensions disagree".into());
        }
        if !bgra.chunks_exact(4).any(|p| p[3] != 0) {
            return Err(
                "Missing or ambiguous raw alpha; legacy color/mask cursor unsupported".into(),
            );
        }
        let raster = Raster::new(w, h)?;
        observe(4, None)?;
        // Draw the owned copy into zeroed owned DIB storage; raw GetDIBits has no proven premultiplication contract.
        unsafe { std::slice::from_raw_parts_mut(raster.bits, raster.len) }.fill(0);
        unsafe {
            DrawIconEx(
                raster.dc, 0, 0, icon.0, w as i32, h as i32, 0, None, DI_NORMAL,
            )?;
            GdiFlush().ok()?;
        }
        let pixels = unsafe { std::slice::from_raw_parts(raster.bits, raster.len) };
        #[cfg(test)]
        {
            let native: Vec<u8> = pixels
                .chunks_exact(4)
                .flat_map(|p| [p[2], p[1], p[0], p[3]])
                .collect();
            observe(5, Some((w, h, &native)))?;
        }
        #[cfg(not(test))]
        observe(5, None)?;
        math::color(w, h, hotspot, pixels)?
    };
    Ok(shape)
}
pub(in crate::recorder) fn decode(icon: &Icon) -> Result<Shape, String> {
    decode_with(icon, |_, _| Ok(())).map_err(|e| e.to_string())
}
