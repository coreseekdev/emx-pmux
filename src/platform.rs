//! Platform utilities: socket naming, daemon spawning, connection helpers.

use interprocess::local_socket::{prelude::*, GenericFilePath, GenericNamespaced, ListenerOptions};
use std::io;

// ── Socket naming ────────────────────────────────────────────────────

/// Runtime directory for pmux files (PID files, Unix sockets).
fn runtime_dir() -> String {
    #[cfg(windows)]
    {
        let base = std::env::var("LOCALAPPDATA")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        format!("{}\\.pmux", base)
    }
    #[cfg(unix)]
    {
        let base = std::env::var("XDG_RUNTIME_DIR")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| "/tmp".into());
        format!("{}/.pmux", base)
    }
}

/// Ensure runtime directory exists.
pub fn ensure_runtime_dir() -> io::Result<()> {
    std::fs::create_dir_all(runtime_dir())
}

/// Socket name / path for the daemon.
fn socket_name() -> String {
    if GenericNamespaced::is_supported() {
        // Windows named pipe or Linux abstract namespace
        "@pmux_daemon".into()
    } else {
        format!("{}/daemon.sock", runtime_dir())
    }
}

fn to_name(s: &str) -> io::Result<interprocess::local_socket::Name<'_>> {
    if GenericNamespaced::is_supported() {
        s.to_ns_name::<GenericNamespaced>()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
    } else {
        s.to_fs_name::<GenericFilePath>()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
    }
}

// ── Connection ───────────────────────────────────────────────────────

/// Connect to the running daemon. Returns a bidirectional stream.
pub fn connect_daemon() -> io::Result<interprocess::local_socket::Stream> {
    let name_str = socket_name();
    let name = to_name(&name_str)?;
    interprocess::local_socket::Stream::connect(name)
}

/// Create a listener for the daemon socket.
pub fn create_listener() -> io::Result<interprocess::local_socket::Listener> {
    ensure_runtime_dir()?;
    let name_str = socket_name();

    // Clean up stale socket on Unix
    #[cfg(unix)]
    if !GenericNamespaced::is_supported() {
        let _ = std::fs::remove_file(&name_str);
    }

    let name = to_name(&name_str)?;
    ListenerOptions::new()
        .name(name)
        .create_sync()
        .map_err(|e| io::Error::new(io::ErrorKind::AddrInUse, e))
}

/// Check if a daemon is reachable.
pub fn is_daemon_running() -> bool {
    connect_daemon().is_ok()
}

// ── PID file ─────────────────────────────────────────────────────────

fn pid_path() -> String {
    format!("{}/daemon.pid", runtime_dir())
}

pub fn write_pid_file() -> io::Result<()> {
    ensure_runtime_dir()?;
    std::fs::write(pid_path(), std::process::id().to_string())
}

pub fn remove_pid_file() {
    let _ = std::fs::remove_file(pid_path());
    // Also clean up socket on Unix
    #[cfg(unix)]
    if !GenericNamespaced::is_supported() {
        let _ = std::fs::remove_file(format!("{}/daemon.sock", runtime_dir()));
    }
}

// ── Daemon spawning ──────────────────────────────────────────────────

/// Spawn the daemon as a detached background process by re-executing
/// the current binary with the internal `--daemon` flag.
pub fn spawn_daemon() -> io::Result<u32> {
    let exe = std::env::current_exe()?;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | DETACHED_PROCESS
        const FLAGS: u32 = 0x08000000 | 0x00000008;
        let child = std::process::Command::new(exe)
            .arg("--daemon")
            .creation_flags(FLAGS)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        Ok(child.id())
    }

    #[cfg(unix)]
    {
        let child = std::process::Command::new(exe)
            .arg("--daemon")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        Ok(child.id())
    }
}

/// Spawn daemon if not already running. Returns Ok(()) when the daemon
/// is accepting connections.
pub fn ensure_daemon() -> io::Result<()> {
    if is_daemon_running() {
        return Ok(());
    }
    spawn_daemon()?;
    // Poll until daemon is reachable.
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(5);
    while start.elapsed() < timeout {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if is_daemon_running() {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "daemon did not start in time",
    ))
}
