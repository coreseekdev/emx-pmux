//! Cross-platform synchronous PTY abstraction.
//!
//! Inspired by rust-pty's trait design, but synchronous (no tokio dependency).
//! - Unix: rustix for PTY allocation (openpt/grantpt/unlockpt)
//! - Windows: ConPTY via windows-sys

mod error;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub use error::{PtyError, PtyResult};

#[cfg(unix)]
pub use unix::{PtyMaster, PtyChild, spawn};
#[cfg(windows)]
pub use windows::{PtyMaster, PtyChild, spawn};

/// Window size for PTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSize {
    pub cols: u16,
    pub rows: u16,
}

impl WindowSize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }
}

impl Default for WindowSize {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

/// Configuration for spawning a PTY process.
#[derive(Debug, Clone)]
pub struct PtyConfig {
    pub program: String,
    pub args: Vec<String>,
    pub working_directory: Option<String>,
    pub env: Vec<(String, String)>,
    pub size: WindowSize,
}

impl Default for PtyConfig {
    fn default() -> Self {
        Self {
            program: default_shell(),
            args: Vec::new(),
            working_directory: None,
            env: Vec::new(),
            size: WindowSize::default(),
        }
    }
}

/// Get the platform default shell.
pub fn default_shell() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
    #[cfg(unix)]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}
