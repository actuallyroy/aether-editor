// Temporary perf instrumentation: appends timing lines to aether-perf.log next to
// the working dir. Windowed subsystem has no console, so stderr is invisible —
// a file lets us inspect timings after interacting. Remove once profiling is done.

use std::io::Write;

pub fn log(msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("aether-perf.log")
    {
        let _ = writeln!(f, "{msg}");
    }
}

/// Scope timer: logs `name: <elapsed>` on drop when it exceeded `min_ms`.
pub struct Probe {
    name: &'static str,
    min: std::time::Duration,
    t0: std::time::Instant,
}

impl Probe {
    pub fn new(name: &'static str, min_ms: u64) -> Probe {
        Probe { name, min: std::time::Duration::from_millis(min_ms), t0: std::time::Instant::now() }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let dt = self.t0.elapsed();
        if dt >= self.min {
            log(&format!("{}: {dt:?}", self.name));
        }
    }
}
