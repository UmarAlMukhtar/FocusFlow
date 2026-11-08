//! Bounded diagnostics for the explicit production qualification fixture only.
use std::{fmt, io::{self, Write}};

#[derive(Clone, Copy)]
pub(super) enum Stage { Geometry, Plan, Defaults, Reservation, Prepare, Arm, Observe }
impl Stage {
    fn label(self) -> &'static str {
        match self {
            Self::Geometry => "geometry", Self::Plan => "plan", Self::Defaults => "defaults",
            Self::Reservation => "reservation", Self::Prepare => "prepare", Self::Arm => "arm",
            Self::Observe => "observe",
        }
    }
}

pub(super) fn report<E: fmt::Display>(stage: Stage, error: E) -> E {
    report_to(&mut io::stderr().lock(), stage, error)
}

// Stop Display formatting at the byte bound instead of allocating the full error first.
struct Message { bytes: [u8; 1024], len: usize }
impl fmt::Write for Message {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let mut count = text.len().min(self.bytes.len() - self.len);
        while !text.is_char_boundary(count) { count -= 1; }
        self.bytes[self.len..self.len+count].copy_from_slice(&text.as_bytes()[..count]);
        self.len += count;
        if count < text.len() { Err(fmt::Error) } else { Ok(()) }
    }
}

fn report_to<E: fmt::Display, W: Write>(writer: &mut W, stage: Stage, error: E) -> E {
    let mut message = Message { bytes: [0; 1024], len: 0 };
    let _ = fmt::write(&mut message, format_args!("{error}"));
    let _ = (|| -> io::Result<()> {
        writer.write_all(b"[production-start-failure] ")?;
        writer.write_all(stage.label().as_bytes())?;
        writer.write_all(b": ")?;
        writer.write_all(&message.bytes[..message.len])?;
        writer.write_all(b"\n")
    })();
    // Try to flush even if writing failed; diagnostics never replace the original error.
    let _ = writer.flush();
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::{Cell, RefCell}, rc::Rc};
    #[derive(Default)]
    struct Saved { bytes: Vec<u8>, events: Vec<&'static str> }
    struct Writer { saved: Rc<RefCell<Saved>>, fail_write: bool }
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_write { return Err(io::Error::new(io::ErrorKind::BrokenPipe, "injected")); }
            self.saved.borrow_mut().bytes.extend_from_slice(bytes); Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> { self.saved.borrow_mut().events.push("flush"); Ok(()) }
    }
    struct Close(Rc<RefCell<Saved>>);
    impl Drop for Close { fn drop(&mut self) { self.0.borrow_mut().events.push("close"); } }

    #[test]
    fn original_error_is_logged_and_flushed_before_close_on_return() {
        fn failing(writer: &mut Writer, error: Box<io::Error>) -> Result<(), Box<io::Error>> {
            let _close = Close(writer.saved.clone());
            Err::<(), _>(error).map_err(|error| report_to(writer, Stage::Prepare, error))?;
            Ok(())
        }
        let saved = Rc::new(RefCell::new(Saved::default()));
        let mut writer = Writer { saved: saved.clone(), fail_write: false };
        let error = Box::new(io::Error::new(io::ErrorKind::Other, "injected prepare error"));
        let identity = &*error as *const io::Error;
        let returned = failing(&mut writer, error).unwrap_err();
        assert_eq!(&*returned as *const io::Error, identity);
        assert_eq!(saved.borrow().bytes, b"[production-start-failure] prepare: injected prepare error\n");
        assert_eq!(saved.borrow().events, ["flush", "close"]);
    }

    #[test]
    fn error_bytes_are_bounded_at_utf8_boundary() {
        let error = format!("a{}", "é".repeat(700));
        let mut bytes = Vec::new();
        let returned = report_to(&mut bytes, Stage::Arm, error.clone());
        assert_eq!(returned, error);
        let text = String::from_utf8(bytes).unwrap();
        let message = text.strip_prefix("[production-start-failure] arm: ").unwrap().strip_suffix('\n').unwrap();
        assert_eq!(message.len(), 1023);
        assert_eq!(message, format!("a{}", "é".repeat(511)));
    }

    #[test]
    fn bounded_formatting_stops_without_rendering_the_whole_error() {
        struct Error(Cell<usize>);
        impl fmt::Display for Error {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                for _ in 0..100_000 { self.0.set(self.0.get()+1); f.write_str("abcd")?; }
                Ok(())
            }
        }
        let mut bytes = Vec::new();
        let error = report_to(&mut bytes, Stage::Plan, Error(Cell::new(0)));
        assert_eq!(error.0.get(), 257);
        assert_eq!(bytes.len(), b"[production-start-failure] plan: ".len()+1024+1);
    }

    #[test]
    fn failed_log_write_still_attempts_flush_and_preserves_original_error() {
        let saved = Rc::new(RefCell::new(Saved::default()));
        let mut writer = Writer { saved: saved.clone(), fail_write: true };
        assert_eq!(report_to(&mut writer, Stage::Observe, "original"), "original");
        assert_eq!(saved.borrow().events, ["flush"]);
    }
}
