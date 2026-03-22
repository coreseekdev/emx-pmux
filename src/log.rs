//! Logging controlled by EMX_TMUX_LOG environment variable.
//!
//! Set `EMX_TMUX_LOG=path/to/file.log` to enable file-based logging.
//! When unset, no logging occurs (zero overhead).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

static LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();

fn init_log() -> Option<Mutex<File>> {
    let path = std::env::var("EMX_TMUX_LOG").ok()?;
    if path.is_empty() {
        return None;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    Some(Mutex::new(file))
}

/// Write a log line to the file specified by EMX_TMUX_LOG.
/// No-op if the variable is unset.
pub fn log(msg: &str) {
    let opt = LOG_FILE.get_or_init(init_log);
    if let Some(mutex) = opt {
        if let Ok(mut f) = mutex.lock() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let _ = writeln!(f, "[{}.{:03}] {}", now.as_secs(), now.subsec_millis(), msg);
            let _ = f.flush();
        }
    }
}

/// Logging macro — usage: `pmux_log!("reader({}): started", name);`
#[macro_export]
macro_rules! pmux_log {
    ($($arg:tt)*) => {
        $crate::log::log(&format!($($arg)*))
    };
}
