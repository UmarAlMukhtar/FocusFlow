pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 360;
pub const MAX_FRAMES: u32 = 600;

pub fn marker(pixels: &[u8], channels: usize) -> Option<u16> {
    if !(channels == 3 || channels == 4) || pixels.len() != WIDTH.checked_mul(HEIGHT)?.checked_mul(channels)? { return None; }
    let mut id = 0u16;
    for bit in 0..16 {
        let i = (160 * WIDTH + 40 + bit * 32) * channels;
        let rgb = &pixels[i..i+3];
        if rgb.iter().all(|c| *c > 210) { id |= 1 << bit; }
        else if !rgb.iter().all(|c| *c < 40) { return None; }
    }
    Some(id)
}
pub fn qpc_100ns(ticks: i64, frequency: i64) -> Option<i64> {
    if frequency <= 0 { return None; }
    i64::try_from(i128::from(ticks).checked_mul(10_000_000)? / i128::from(frequency)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_decodes_independent_literal_and_rejects_damage() {
        let mut pixels = vec![96; WIDTH * HEIGHT * 4];
        // Literal ID 0xa5 = bits 0,2,5,7, unrelated to decoder implementation.
        for bit in 0..16 {
            let c = if [0,2,5,7].contains(&bit) { 240 } else { 12 };
            let i = (160 * WIDTH + 40 + bit * 32) * 4;
            pixels[i..i+3].fill(c);
        }
        assert_eq!(marker(&pixels,4), Some(165));
        pixels[(160*WIDTH+40)*4] = 120;
        assert_eq!(marker(&pixels,4),None);
        assert_eq!(marker(&pixels[..20],4),None);
    }
    #[test]
    fn qpc_conversion_uses_wide_checked_signed_math() {
        assert_eq!(qpc_100ns(12_345_678,3_000_000),Some(41_152_260));
        assert_eq!(qpc_100ns(-3,10_000_000),Some(-3));
        assert_eq!(qpc_100ns(i64::MAX,1),None);
        assert_eq!(qpc_100ns(1,0),None);
    }
}
