//! Platform utilities: socket naming, daemon spawning, connection helpers.

use std::io;

// ── Socket naming ────────────────────────────────────────────────────

/// Runtime directory for pmux files (PID files, Unix sockets).
pub fn runtime_dir() -> String {
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

/// Socket path for the daemon.
pub fn socket_path() -> String {
    #[cfg(unix)]
    {
        format!("{}/daemon.sock", runtime_dir())
    }
    #[cfg(windows)]
    {
        // Named pipe path on Windows
        r"\\.\pipe\pmux_daemon".to_string()
    }
}

// ── Connection (async) ───────────────────────────────────────────────

/// Async connect to the running daemon.
#[cfg(unix)]
pub async fn connect_daemon() -> io::Result<tokio::net::UnixStream> {
    let path = socket_path();
    tokio::net::UnixStream::connect(&path).await
}

#[cfg(windows)]
pub async fn connect_daemon() -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    let path = socket_path();
    tokio::net::windows::named_pipe::ClientOptions::new().open(&path)
}

/// Create an async listener for the daemon socket.
#[cfg(unix)]
pub async fn create_listener() -> io::Result<tokio::net::UnixListener> {
    ensure_runtime_dir()?;
    let path = socket_path();
    // Clean up stale socket
    let _ = std::fs::remove_file(&path);
    tokio::net::UnixListener::bind(&path)
}

#[cfg(windows)]
pub fn create_pipe_instance() -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    let path = socket_path();
    tokio::net::windows::named_pipe::ServerOptions::new()
        .first_pipe_instance(true)
        .create(&path)
}

#[cfg(windows)]
pub fn create_next_pipe_instance() -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    let path = socket_path();
    tokio::net::windows::named_pipe::ServerOptions::new()
        .first_pipe_instance(false)
        .create(&path)
}

/// Check if a daemon is reachable (sync, used by spawn logic).
pub fn is_daemon_running() -> bool {
    // Quick sync check by trying to connect
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::connect(socket_path()).is_ok()
    }
    #[cfg(windows)]
    {
        // Try to open the named pipe
        use std::fs::OpenOptions;
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(socket_path())
            .is_ok()
    }
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
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(socket_path());
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
pub async fn ensure_daemon() -> io::Result<()> {
    if is_daemon_running() {
        return Ok(());
    }
    spawn_daemon()?;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(5);
    while start.elapsed() < timeout {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if is_daemon_running() {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "daemon did not start in time",
    ))
}
