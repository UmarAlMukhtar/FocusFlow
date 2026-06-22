use std::{
    env,
    error::Error,
    ffi::c_void,
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    encoder::{
        AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder,
        VideoSettingsSubType,
    },
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

use windows::Win32::{
    Foundation::{HWND, RECT},
    System::Threading::GetCurrentProcessId,
    UI::WindowsAndMessaging::{
        GetClientRect, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic, IsWindow,
        IsWindowVisible, IsZoomed, GWL_EXSTYLE, GWL_STYLE, WINDOW_LONG_PTR_INDEX, WS_CHILD,
        WS_EX_TOOLWINDOW,
    },
};

type PocResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const DEFAULT_OUTPUT_FILE: &str = "screen.mp4";
const DEFAULT_DURATION_SECONDS: u64 = 8;
const TARGET_FPS: u32 = 60;
const TARGET_BITRATE: u32 = 12_000_000;
const WINDOW_SIZE_LOG_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
struct CaptureFlags {
    output_path: PathBuf,
    width: u32,
    height: u32,
    hwnd: usize,
}

struct WindowCapturePoc {
    encoder: Option<VideoEncoder>,
    started_at: Instant,
    frame_count: u64,
    hwnd: usize,
    encoder_width: u32,
    encoder_height: u32,
    last_frame_size: Option<(u32, u32)>,
    last_window_size_log: Instant,
}

impl WindowCapturePoc {
    fn finish_encoder(&mut self) -> PocResult<()> {
        if let Some(encoder) = self.encoder.take() {
            encoder.finish()?;
        }

        Ok(())
    }
}

impl GraphicsCaptureApiHandler for WindowCapturePoc {
    type Flags = CaptureFlags;
    type Error = Box<dyn Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        println!(
            "Encoder dimensions locked at startup: {}x{}",
            ctx.flags.width, ctx.flags.height
        );
        println!(
            "windows-capture resize callback: no public resize callback is exposed by GraphicsCaptureApiHandler; resize is inferred from Frame::width()/height()."
        );

        let video_settings = VideoSettingsBuilder::new(ctx.flags.width, ctx.flags.height)
            .sub_type(VideoSettingsSubType::H264)
            .frame_rate(TARGET_FPS)
            .bitrate(TARGET_BITRATE);

        let encoder = VideoEncoder::new(
            video_settings,
            AudioSettingsBuilder::default().disabled(true),
            ContainerSettingsBuilder::default(),
            &ctx.flags.output_path,
        )?;

        Ok(Self {
            encoder: Some(encoder),
            started_at: Instant::now(),
            frame_count: 0,
            hwnd: ctx.flags.hwnd,
            encoder_width: ctx.flags.width,
            encoder_height: ctx.flags.height,
            last_frame_size: None,
            last_window_size_log: Instant::now(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        self.log_frame_size_change(frame);
        self.log_window_size_periodically();

        if let Some(encoder) = self.encoder.as_mut() {
            encoder.send_frame(frame)?;
            self.frame_count = self.frame_count.saturating_add(1);
        }

        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        self.finish_encoder()
    }
}

impl WindowCapturePoc {
    fn log_frame_size_change(&mut self, frame: &Frame) {
        let frame_size = (frame.width(), frame.height());

        if self.last_frame_size != Some(frame_size) {
            println!(
                "Frame size changed: {}x{}; encoder remains {}x{}",
                frame_size.0, frame_size.1, self.encoder_width, self.encoder_height
            );
            self.last_frame_size = Some(frame_size);
        }
    }

    fn log_window_size_periodically(&mut self) {
        if self.last_window_size_log.elapsed() < WINDOW_SIZE_LOG_INTERVAL {
            return;
        }

        let window = Window::from_raw_hwnd(self.hwnd as *mut c_void);
        println!(
            "Window size probe: width={} height={}",
            diagnostic_result(window.width()),
            diagnostic_result(window.height())
        );
        self.last_window_size_log = Instant::now();
    }
}

fn main() -> PocResult<()> {
    let args = CliArgs::parse()?;
    let window = Window::from_raw_hwnd(args.hwnd as *mut c_void);

    println!("FocusFlow windows-capture proof of concept");
    print_hwnd_diagnostics(args.hwnd, &window);
    print_capturable_windows();

    if !window.is_valid() {
        println!("windows-capture rejection:");
        println!("  Window::is_valid() returned false.");
        println!("  Reason: {}", invalid_capture_reason(args.hwnd));
        return Err(format!(
            "HWND 0x{:X} is not a valid capturable window according to windows-capture",
            args.hwnd
        )
        .into());
    }

    let width = u32::try_from(window.width()?)?;
    let height = u32::try_from(window.height()?)?;

    if width == 0 || height == 0 {
        return Err(format!("HWND 0x{:X} resolved to an empty window", args.hwnd).into());
    }

    if let Some(parent) = args
        .output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    println!("Size: {}x{}", width, height);
    println!("Duration: {} seconds", args.duration.as_secs());
    println!("Output: {}", args.output_path.display());

    let settings = Settings::new(
        window,
        CursorCaptureSettings::Default,
        DrawBorderSettings::Default,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        CaptureFlags {
            output_path: args.output_path,
            width,
            height,
            hwnd: args.hwnd,
        },
    );

    let capture = match WindowCapturePoc::start_free_threaded(settings) {
        Ok(capture) => capture,
        Err(error) => {
            println!("windows-capture rejection:");
            println!("  start_free_threaded() failed: {error}");
            println!("  Reason: {}", invalid_capture_reason(args.hwnd));
            return Err(Box::new(error));
        }
    };
    thread::sleep(args.duration);

    let callback = capture.callback();
    let (frames, elapsed) = {
        let mut capture_state = callback.lock();
        capture_state.finish_encoder()?;
        (
            capture_state.frame_count,
            capture_state.started_at.elapsed().as_secs_f32(),
        )
    };

    capture.stop()?;
    println!("Frames encoded: {frames}");
    println!("Elapsed: {elapsed:.2} seconds");
    println!("Recording complete.");

    Ok(())
}

fn print_hwnd_diagnostics(hwnd: usize, window: &Window) {
    let native_hwnd = HWND(hwnd as *mut c_void);
    let process_id = process_id_for_hwnd(native_hwnd);
    let current_process_id = unsafe { GetCurrentProcessId() };

    println!("Selected HWND diagnostics:");
    println!("  HWND: 0x{hwnd:X}");
    println!("  Window title: {}", diagnostic_result(window.title()));
    println!("  Process ID: {}", format_optional_u32(process_id));
    println!("  Current process ID: {current_process_id}");
    println!("  IsWindow(): {}", unsafe {
        IsWindow(Some(native_hwnd)).as_bool()
    });
    println!("  IsWindowVisible(): {}", unsafe {
        IsWindowVisible(native_hwnd).as_bool()
    });
    println!("  IsIconic() (minimized): {}", unsafe {
        IsIconic(native_hwnd).as_bool()
    });
    println!("  IsZoomed() (maximized): {}", unsafe {
        IsZoomed(native_hwnd).as_bool()
    });
    println!(
        "  windows-capture Window::is_valid(): {}",
        window.is_valid()
    );
    println!("  GetClientRect(): {}", format_client_rect(native_hwnd));
    println!(
        "  Has WS_CHILD: {}",
        has_window_style(native_hwnd, GWL_STYLE, WS_CHILD.0)
    );
    println!(
        "  Has WS_EX_TOOLWINDOW: {}",
        has_window_style(native_hwnd, GWL_EXSTYLE, WS_EX_TOOLWINDOW.0)
    );
    println!(
        "  windows-capture width(): {}",
        diagnostic_result(window.width())
    );
    println!(
        "  windows-capture height(): {}",
        diagnostic_result(window.height())
    );
}

fn print_capturable_windows() {
    println!("windows-capture capturable windows:");

    match Window::enumerate() {
        Ok(windows) if windows.is_empty() => {
            println!("  No capturable windows returned by Window::enumerate().");
        }
        Ok(windows) => {
            println!("  Count: {}", windows.len());
            for (index, window) in windows.iter().enumerate() {
                let hwnd = window.as_raw_hwnd() as usize;
                let title = window
                    .title()
                    .unwrap_or_else(|error| format!("title error: {error}"));
                let process_name = window
                    .process_name()
                    .unwrap_or_else(|error| format!("process error: {error}"));
                let width = window
                    .width()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|error| format!("width error: {error}"));
                let height = window
                    .height()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|error| format!("height error: {error}"));

                println!(
                    "  [{index}] hwnd=0x{hwnd:X} title=\"{title}\" process=\"{process_name}\" size={width}x{height}"
                );
            }
        }
        Err(error) => {
            println!("  Window::enumerate() failed: {error}");
        }
    }
}

fn invalid_capture_reason(hwnd: usize) -> String {
    let native_hwnd = HWND(hwnd as *mut c_void);

    if !unsafe { IsWindow(Some(native_hwnd)).as_bool() } {
        return "Win32 IsWindow() returned false; the HWND does not identify a live window."
            .to_string();
    }

    if !unsafe { IsWindowVisible(native_hwnd).as_bool() } {
        return "Win32 IsWindowVisible() returned false; windows-capture rejects hidden windows."
            .to_string();
    }

    if let Some(process_id) = process_id_for_hwnd(native_hwnd) {
        let current_process_id = unsafe { GetCurrentProcessId() };
        if process_id == current_process_id {
            return "The HWND belongs to this proof-of-concept process; windows-capture rejects its own process windows."
                .to_string();
        }
    }

    if !client_rect_available(native_hwnd) {
        return "GetClientRect() failed; windows-capture requires a readable client rectangle."
            .to_string();
    }

    if has_window_style(native_hwnd, GWL_EXSTYLE, WS_EX_TOOLWINDOW.0) {
        return "The window has WS_EX_TOOLWINDOW; windows-capture rejects tool windows."
            .to_string();
    }

    if has_window_style(native_hwnd, GWL_STYLE, WS_CHILD.0) {
        return "The window has WS_CHILD; windows-capture rejects child windows.".to_string();
    }

    if unsafe { IsIconic(native_hwnd).as_bool() } {
        return "The window is minimized. The windows-capture source does not explicitly check IsIconic(), but minimized windows are usually not useful capture targets."
            .to_string();
    }

    "No local Win32 reason was detected. The HWND may be protected, owned by a special/system surface, or unsupported by Windows Graphics Capture."
        .to_string()
}

fn process_id_for_hwnd(hwnd: HWND) -> Option<u32> {
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };

    (process_id != 0).then_some(process_id)
}

fn client_rect_available(hwnd: HWND) -> bool {
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect).is_ok() }
}

fn format_client_rect(hwnd: HWND) -> String {
    let mut rect = RECT::default();

    match unsafe { GetClientRect(hwnd, &mut rect) } {
        Ok(()) => format!(
            "ok left={} top={} right={} bottom={} width={} height={}",
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            rect.right - rect.left,
            rect.bottom - rect.top
        ),
        Err(error) => format!("error: {error}"),
    }
}

fn has_window_style(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX, style_flag: u32) -> bool {
    let styles = unsafe { GetWindowLongPtrW(hwnd, index) };
    (styles & isize::try_from(style_flag).unwrap_or_default()) != 0
}

fn diagnostic_result<T, E>(result: Result<T, E>) -> String
where
    T: ToString,
    E: ToString,
{
    result
        .map(|value| value.to_string())
        .unwrap_or_else(|error| format!("error: {}", error.to_string()))
}

fn format_optional_u32(value: Option<u32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[derive(Debug)]
struct CliArgs {
    hwnd: usize,
    output_path: PathBuf,
    duration: Duration,
}

impl CliArgs {
    fn parse() -> PocResult<CliArgs> {
        let mut args = env::args().skip(1);
        let hwnd = args.next().ok_or_else(usage)?.parse_hwnd()?;
        let output_path = args
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT_FILE));
        let duration_seconds = args
            .next()
            .map(|value| value.parse::<u64>())
            .transpose()?
            .unwrap_or(DEFAULT_DURATION_SECONDS);

        if duration_seconds == 0 {
            return Err("Duration must be at least 1 second".into());
        }

        Ok(CliArgs {
            hwnd,
            output_path,
            duration: Duration::from_secs(duration_seconds),
        })
    }
}

trait ParseHwnd {
    fn parse_hwnd(self) -> PocResult<usize>;
}

impl ParseHwnd for String {
    fn parse_hwnd(self) -> PocResult<usize> {
        let value = self.trim();
        let value = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
            .unwrap_or(value);

        usize::from_str_radix(value, 16)
            .or_else(|_| self.trim().parse::<usize>())
            .map_err(|_| format!("Invalid HWND: {}", self).into())
    }
}

fn usage() -> Box<dyn Error + Send + Sync> {
    format!(
        "Usage: cargo run --bin window_capture_poc -- <hwnd> [output_path] [duration_seconds]\n\
         Example: cargo run --bin window_capture_poc -- 0x001A0B4E screen.mp4 8"
    )
    .into()
}
