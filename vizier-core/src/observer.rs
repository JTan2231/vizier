use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::io::{self, Write};
use std::sync::Arc;

struct TeeWriter<W: Write> {
    upstream: W,
    capture: Option<Arc<Mutex<Vec<u8>>>>,
}

impl<W: Write> TeeWriter<W> {
    fn new(upstream: W) -> Self {
        Self {
            upstream,
            capture: None,
        }
    }

    fn set_capture(&mut self, buf: Option<Arc<Mutex<Vec<u8>>>>) {
        self.capture = buf;
    }
}

impl<W: Write> Write for TeeWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.upstream.write(buf)?;
        if let Some(cap) = &self.capture {
            cap.lock().extend_from_slice(&buf[..n]);
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.upstream.flush()
    }
}

pub struct Console {
    out: TeeWriter<io::Stdout>,
    err: TeeWriter<io::Stderr>,
    out_cap: Arc<Mutex<Vec<u8>>>,
    err_cap: Arc<Mutex<Vec<u8>>>,
}

impl Console {
    fn new() -> Self {
        let out_cap = Arc::new(Mutex::new(Vec::new()));
        let err_cap = Arc::new(Mutex::new(Vec::new()));
        let mut out = TeeWriter::new(io::stdout());
        let mut err = TeeWriter::new(io::stderr());
        out.set_capture(Some(out_cap.clone()));
        err.set_capture(Some(err_cap.clone()));
        Self {
            out,
            err,
            out_cap,
            err_cap,
        }
    }

    fn enable_capture(&mut self, enable: bool) {
        let out = if enable {
            Some(self.out_cap.clone())
        } else {
            None
        };
        let err = if enable {
            Some(self.err_cap.clone())
        } else {
            None
        };
        self.out.set_capture(out);
        self.err.set_capture(err);
    }

    fn take_stdout(&self) -> String {
        let mut v = self.out_cap.lock();
        let s = String::from_utf8_lossy(&v).into_owned();
        v.clear();
        s
    }

    fn take_stderr(&self) -> String {
        let mut v = self.err_cap.lock();
        let s = String::from_utf8_lossy(&v).into_owned();
        v.clear();
        s
    }
}

static CONSOLE: Lazy<Mutex<Console>> = Lazy::new(|| Mutex::new(Console::new()));

pub struct CaptureGuard {
    prev_enabled: bool,
}

impl CaptureGuard {
    pub fn start() -> Self {
        let mut c = CONSOLE.lock();
        let was_enabled = !c.out_cap.lock().is_empty() || !c.err_cap.lock().is_empty();
        c.enable_capture(true);
        Self {
            prev_enabled: was_enabled,
        }
    }
    pub fn take_both(&self) -> (String, String) {
        let c = CONSOLE.lock();
        (c.take_stdout(), c.take_stderr())
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        let mut c = CONSOLE.lock();
        c.enable_capture(self.prev_enabled);
    }
}

// Print helpers mirroring println!/eprintln!
#[macro_export]
macro_rules! cprintln {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let mut c = $crate::CONSOLE.lock();
        let _ = writeln!(&mut c.out, "{}", format!($($arg)*));
        let _ = c.out.flush();
    }};
}

#[macro_export]
macro_rules! ceprintln {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let mut c = $crate::CONSOLE.lock();
        let _ = writeln!(&mut c.err, "{}", format!($($arg)*));
        let _ = c.err.flush();
    }};
}

pub fn take_stdout() -> String {
    CONSOLE.lock().take_stdout()
}

pub fn take_stderr() -> String {
    CONSOLE.lock().take_stderr()
}
