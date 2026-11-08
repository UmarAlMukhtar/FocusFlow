//! Owned resource raster conversion; animation phase remains unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub width: u32,
    pub height: u32,
    pub hotspot: [u32; 2],
    pub rgba: Vec<u8>,
}
fn shape(w: u32, h: u32, hotspot: [u32; 2]) -> Result<Shape, &'static str> {
    if w == 0 || h == 0 || w > 512 || h > 512 || hotspot[0] >= w || hotspot[1] >= h {
        return Err("Invalid cursor dimensions/hotspot");
    }
    Ok(Shape {
        width: w,
        height: h,
        hotspot,
        rgba: Vec::with_capacity(w as usize * h as usize * 4),
    })
}
pub fn color(w: u32, h: u32, hotspot: [u32; 2], bgra: &[u8]) -> Result<Shape, &'static str> {
    let mut s = shape(w, h, hotspot)?;
    if bgra.len() != w as usize * h as usize * 4 || !bgra.chunks_exact(4).any(|p| p[3] != 0) {
        return Err("Missing or ambiguous alpha");
    }
    for p in bgra.chunks_exact(4) {
        let a = u32::from(p[3]);
        if a == 0 {
            s.rgba.extend_from_slice(&[0; 4]);
            continue;
        }
        if p[..3].iter().any(|v| u32::from(*v) > a) {
            return Err("Not premultiplied alpha");
        }
        for v in [p[2], p[1], p[0]] {
            s.rgba.push(((u32::from(v) * 255 + a / 2) / a) as u8);
        }
        s.rgba.push(p[3]);
    }
    Ok(s)
}
pub fn monochrome(
    w: u32,
    h: u32,
    hotspot: [u32; 2],
    and: &[bool],
    xor: &[bool],
) -> Result<Shape, &'static str> {
    let mut s = shape(w, h, hotspot)?;
    if and.len() != w as usize * h as usize || xor.len() != and.len() {
        return Err("Truncated masks");
    }
    for (&a, &x) in and.iter().zip(xor) {
        if a && x {
            return Err("Destination-dependent XOR is unsupported");
        }
        s.rgba.extend_from_slice(if a {
            &[0, 0, 0, 0]
        } else if x {
            &[255; 4]
        } else {
            &[0, 0, 0, 255]
        });
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alpha_conversion_and_xor_rejection_preserve_the_qualified_resource_contract() {
        assert_eq!(
            color(2, 1, [1, 0], &[0, 0, 64, 128, 99, 77, 66, 0])
                .unwrap()
                .rgba,
            [128, 0, 0, 128, 0, 0, 0, 0]
        );
        assert!(color(1, 1, [0, 0], &[255, 0, 0, 1]).is_err());
        assert!(color(1, 1, [0, 0], &[0; 4]).is_err());
        assert!(color(1, 1, [1, 0], &[0, 0, 0, 255]).is_err());
        assert_eq!(
            monochrome(3, 1, [1, 0], &[false, false, true], &[false, true, false])
                .unwrap()
                .rgba,
            [0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 0]
        );
        assert!(monochrome(1, 1, [0, 0], &[true], &[true]).is_err());
    }
}
