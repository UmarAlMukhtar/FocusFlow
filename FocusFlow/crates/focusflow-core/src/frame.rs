//! Validated canvas geometry and composition shared by every renderer.
use crate::{project::{require, CanvasFormat, FrameBackground, FrameStyle}, CoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameGeometry {
    pub width: u32, pub height: u32,
    pub content_width: u32, pub content_height: u32,
    pub content_x: u32, pub content_y: u32, pub corner_radius: u32,
}

/// Presets keep the source longest edge; content fits within padding without stretching.
pub fn geometry(style: &FrameStyle, source_width: u32, source_height: u32) -> CoreResult<FrameGeometry> {
    crate::project::validate_video(source_width, source_height, 1)?;
    require(style.padding_px <= 256, "frameStyle.paddingPx", "Padding must be between 0 and 256 pixels")?;
    require(style.corner_radius_px <= 128, "frameStyle.cornerRadiusPx", "Corner radius must be between 0 and 128 pixels")?;
    let color = |value: &str| require(value.len() == 7 && value.starts_with('#') && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit),
        "frameStyle.background", "Colors must be exact #RRGGBB strings");
    match &style.background {
        FrameBackground::Solid { color: value } => color(value)?,
        FrameBackground::Gradient { start, end } => { color(start)?; color(end)?; }
    }
    let even = |value: u32| (value / 2 * 2).max(2);
    let sw = even(source_width); let sh = even(source_height); let longest = sw.max(sh);
    let (width, height) = match style.canvas {
        CanvasFormat::Source => (sw, sh),
        CanvasFormat::Landscape => (longest, even(longest * 9 / 16)),
        CanvasFormat::Portrait => (even(longest * 9 / 16), longest),
        CanvasFormat::Square => (longest, longest),
    };
    let padding = style.padding_px * 2;
    require(width > padding + 1 && height > padding + 1, "frameStyle.paddingPx", "Padding leaves no nonempty even-sized content region")?;
    let aw = width - padding; let ah = height - padding;
    let scale = (aw as f64 / sw as f64).min(ah as f64 / sh as f64);
    let content_width = ((sw as f64 * scale / 2.0).round() as u32 * 2).min(aw / 2 * 2);
    let content_height = ((sh as f64 * scale / 2.0).round() as u32 * 2).min(ah / 2 * 2);
    require(content_width >= 2 && content_height >= 2, "frameStyle", "Canvas cannot fit a nonempty even-sized source image")?;
    Ok(FrameGeometry { width, height, content_width, content_height,
        content_x: (width - content_width) / 2, content_y: (height - content_height) / 2,
        corner_radius: style.corner_radius_px.min(content_width / 2).min(content_height / 2) })
}

/// Compose after capture-space camera/click rendering. The identity path adds no filters.
pub(crate) fn composition(style: &FrameStyle, source_width: u32, source_height: u32, fps: u32, duration_us: u64) -> CoreResult<String> {
    let g = geometry(style, source_width, source_height)?;
    let duration = (duration_us * fps as u64).div_ceil(1_000_000) as f64 / fps as f64;
    let w = g.width; let h = g.height;
    let cw = g.content_width; let ch = g.content_height; let x = g.content_x; let y = g.content_y;
    let background = match &style.background {
        FrameBackground::Solid { color } => format!("color=c={color}:s={w}x{h}:r={fps}:d={duration:.9},format=gbrp"),
        FrameBackground::Gradient { start, end } => {
            let channel = |offset| {
                // geometry validates six ASCII hexadecimal digits before these slices are used.
                let a = u8::from_str_radix(&start[offset..offset + 2], 16).unwrap_or(0);
                let b = u8::from_str_radix(&end[offset..offset + 2], 16).unwrap_or(0);
                format!("{a}+({b}-{a})*(X/({w}-1)+Y/({h}-1))/2")
            };
            format!("color=c=black:s={w}x{h}:r={fps}:d={duration:.9},format=gbrp,geq=r='{}':g='{}':b='{}'", channel(1), channel(3), channel(5))
        }
    };
    let mask = rounded_mask(cw, ch, g.corner_radius, "X", "Y");
    let mut graph = format!("{background}[frame_bg];\n[frame_input]scale={cw}:{ch},format=gbrap,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='255*({mask})'[frame_content]");
    let backdrop = if style.shadow {
        let shadow_mask = rounded_mask(cw, ch, g.corner_radius, &format!("(X-{x})"), &format!("(Y-{y}-8)"));
        graph.push_str(&format!(";\ncolor=c=black@0:s={w}x{h}:r={fps}:d={duration:.9},format=gbrap,geq=r=0:g=0:b=0:a='100*({shadow_mask})',gblur=sigma=6:planes=8[frame_shadow];\n[frame_bg][frame_shadow]overlay=shortest=1:format=auto[frame_backdrop]"));
        "frame_backdrop"
    } else { "frame_bg" };
    graph.push_str(&format!(";\n[{backdrop}][frame_content]overlay=x={x}:y={y}:shortest=1:format=auto,format=yuv420p,setsar=1[v]"));
    Ok(graph)
}

fn rounded_mask(width: u32, height: u32, radius: u32, x: &str, y: &str) -> String {
    if radius == 0 { return format!("between({x},0,{})*between({y},0,{})", width - 1, height - 1); }
    // Signed distance to a rounded rectangle gives a one-pixel antialiased edge.
    let cx = (width - 1) as f64 / 2.0; let cy = (height - 1) as f64 / 2.0;
    format!("between({x},0,{})*between({y},0,{})*clip({radius}+0.5-hypot(max(abs({x}-{cx})-({cx}-{radius}),0),max(abs({y}-{cy})-({cy}-{radius}),0)),0,1)", width - 1, height - 1)
}
