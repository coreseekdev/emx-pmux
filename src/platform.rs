//! Platform abstraction layer
//!
//! Provides platform-specific implementations for Windows and POSIX systems.
//!
//! # Architecture
//!
//! - **Windows**: Uses `CREATE_NO_WINDOW` / `DETACHED_PROCESS` for daemon mode
//! - **POSIX**: Uses traditional `fork` + `setsid` for daemon mode

// Platform-specific modules
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(unix)]
mod posix;
#[cfg(unix)]
pub use posix::*;

// Re-export common path functions for use in main.rs
#[cfg(windows)]
pub use windows::{pmux_dir, port_file_path, key_file_path};

#[cfg(unix)]
pub use posix::{pmux_dir, socket_file_path, lock_file_path};

/// Platform detection
pub const PLATFORM: &str = if cfg!(windows) {
    "windows"
} else if cfg!(target_os = "linux") {
    "linux"
} else if cfg!(target_os = "macos") {
    "macos"
} else {
    "unknown"
};

/// Daemon configuration
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Session name
    pub session_name: String,

    /// Working directory
    pub work_dir: Option<String>,

    /// Environment variables to pass
    pub env: Vec<(String, String)>,

    /// Initial window size (optional, for warm start)
    pub init_size: Option<(u16, u16)>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        DaemonConfig {
            session_name: "default".to_string(),
            work_dir: None,
            env: Vec::new(),
            init_size: None,
        }
    }
}

/// Daemon handle for managing the background process
#[derive(Debug)]
pub struct DaemonHandle {
    /// Process ID
    pub pid: u32,

    /// Session name
    pub session_name: String,
}

/// Spawn a new daemon process
///
/// This creates a new background process without any console/terminal attachment.
pub fn spawn_daemon(config: DaemonConfig) -> Result<DaemonHandle, String> {
    spawn_daemon_impl(config)
}

/// Check if a daemon is running for the given session
pub fn is_daemon_running(session_name: &str) -> bool {
    is_daemon_running_impl(session_name)
}

/// Stop a daemon by session name
pub fn stop_daemon(session_name: &str) -> Result<(), String> {
    stop_daemon_impl(session_name)
}

/// Get list of running daemons
pub fn list_daemons() -> Vec<String> {
    list_daemons_impl()
}
