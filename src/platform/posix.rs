//! POSIX-specific daemon implementation
//!
//! Uses traditional UNIX double-fork technique for daemonization.
//!
//! # Daemonization Process
//!
//! 1. Fork - parent exits, child continues
//! 2. setsid - create new session, become session leader
//! 3. Fork again - child exits, grandchild continues
//! 4. Grandchild is now:
//!    - Not a session leader (won't acquire TTY)
//!    - In its own process group
//!    - Detached from controlling terminal

use std::env;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use super::{DaemonConfig, DaemonHandle};

/// pmux directory for Unix
const PMUX_DIR: &str = "/.pmux";

/// Get the pmux directory path
pub fn pmux_dir() -> String {
    let home = env::var("HOME")
        .or_else(|_| env::var("USER"))
        .unwrap_or_else(|_| "/tmp".to_string());
    format!("{}{}", home, PMUX_DIR)
}

/// Get the socket file path for a session (Unix domain socket)
pub fn socket_file_path(session_name: &str) -> String {
    format!("{}/{}.sock", pmux_dir(), session_name)
}

/// Get the lock file path for a session (PID file)
pub fn lock_file_path(session_name: &str) -> String {
    format!("{}/{}.pid", pmux_dir(), session_name)
}

/// POSIX-specific implementation: spawn daemon using double-fork
pub fn spawn_daemon_impl(config: DaemonConfig) -> Result<DaemonHandle, String> {
    // Ensure directory exists
    let dir = pmux_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create pmux dir: {}", e))?;

    let exe = env::current_exe()
        .map_err(|e| format!("Failed to get executable path: {}", e))?;

    // Use double-fork technique through self-execution
    // First exec with --daemon-fork flag
    let mut cmd = Command::new(&exe);

    cmd.arg("daemon")
        .arg("-s")
        .arg(&config.session_name)
        .arg("--daemonized");  // Marker that we're already daemonized

    // Optional: initial size
    if let Some((w, h)) = config.init_size {
        cmd.arg("-x").arg(w.to_string())
            .arg("-y").arg(h.to_string());
    }

    // Detach: double fork happens inside the daemon process
    unsafe {
        cmd.pre_exec(|| {
            // Fork - first fork
            match nix::unistd::fork()? {
                nix::unistd::ForkResult::Parent { .. } => {
                    // Parent exits immediately
                    std::process::exit(0);
                }
                nix::unistd::ForkResult::Child => {
                    // Child continues
                }
            }

            // Create new session
            nix::unistd::setsid().map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

            // Fork again - second fork
            match nix::unistd::fork()? {
                nix::unistd::ForkResult::Parent { .. } => {
                    // Intermediate process exits
                    std::process::exit(0);
                }
                nix::unistd::ForkResult::Child => {
                    // Grandchild becomes the daemon
                }
            }

            // Change working directory to root (or specified)
            let work_dir = config.work_dir.as_deref().unwrap_or("/");
            env::set_current_dir(work_dir)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

            // Clear umask
            nix::sys::stat::umask(nix::sys::stat::Mode::empty());

            // Redirect standard file descriptors to /dev/null
            let dev_null = OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/null")?;

            let _ = nix::unistd::dup2(dev_null.as_raw_fd(), 0); // stdin
            let _ = nix::unistd::dup2(dev_null.as_raw_fd(), 1); // stdout
            let _ = nix::unistd::dup2(dev_null.as_raw_fd(), 2); // stderr

            Ok(())
        });
    }

    // Spawn will trigger the pre_exec which does the double-fork
    let child = cmd.spawn()
        .map_err(|e| format!("Failed to spawn daemon: {}", e))?;

    // Note: Due to double-fork, the returned child is actually
    // the intermediate process that will exit quickly.
    // The actual daemon PID is written to the .pid file.

    // Wait a bit for daemon to start and write PID file
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Read PID from lock file
    if let Ok(pid_str) = std::fs::read_to_string(lock_file_path(&config.session_name)) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            return Ok(DaemonHandle {
                pid,
                session_name: config.session_name,
            });
        }
    }

    // Fallback: return the child's PID (less accurate)
    Ok(DaemonHandle {
        pid: child.id(),
        session_name: config.session_name,
    })
}

/// POSIX-specific: check if daemon is running
pub fn is_daemon_running_impl(session_name: &str) -> bool {
    let lock_path = lock_file_path(session_name);

    // Check if lock file exists
    if !std::path::Path::new(&lock_path).exists() {
        return false;
    }

    // Read PID and check if process exists
    if let Ok(pid_str) = std::fs::read_to_string(&lock_path) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            return is_process_alive(pid);
        }
    }

    false
}

/// Check if a process with given PID is alive
fn is_process_alive(pid: u32) -> bool {
    use nix::sys::signal::{self, Signal};
    use nix::unistd::Pid;

    // Send signal 0 to check if process exists
    signal::kill(Pid::from_raw(pid as i32), None).is_ok()
}

/// POSIX-specific: stop daemon by sending SIGTERM
pub fn stop_daemon_impl(session_name: &str) -> Result<(), String> {
    let lock_path = lock_file_path(session_name);

    // Read PID from lock file
    let pid_str = std::fs::read_to_string(&lock_path)
        .map_err(|e| format!("Failed to read lock file: {}", e))?;

    let pid: u32 = pid_str.trim()
        .parse()
        .map_err(|e| format!("Invalid PID in lock file: {}", e))?;

    // Send SIGTERM
    use nix::sys::signal;
    use nix::unistd::Pid;

    signal::kill(Pid::from_raw(pid as i32), signal::Signal::SIGTERM)
        .map_err(|e| format!("Failed to send SIGTERM: {}", e))?;

    // Wait a bit for graceful shutdown
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Check if still running, if so SIGKILL
    if is_process_alive(pid) {
        signal::kill(Pid::from_raw(pid as i32), signal::Signal::SIGKILL)
            .map_err(|e| format!("Failed to send SIGKILL: {}", e))?;
    }

    // Clean up lock file and socket
    let _ = std::fs::remove_file(&lock_path);
    let _ = std::fs::remove_file(socket_file_path(session_name));

    Ok(())
}

/// POSIX-specific: list running daemons
pub fn list_daemons_impl() -> Vec<String> {
    let mut daemons = Vec::new();
    let dir = pmux_dir();

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "pid").unwrap_or(false) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    // Verify it's actually running
                    if is_daemon_running_impl(stem) {
                        daemons.push(stem.to_string());
                    } else {
                        // Clean up stale lock file
                        let _ = std::fs::remove_file(&path);
                        let sock_path = socket_file_path(stem);
                        let _ = std::fs::remove_file(&sock_path);
                    }
                }
            }
        }
    }

    daemons.sort();
    daemons
}

/// POSIX: Write PID file (called by daemon process)
pub fn write_pid_file(session_name: &str) -> io::Result<()> {
    let pid = std::process::id();
    let lock_path = lock_file_path(session_name);
    std::fs::write(&lock_path, format!("{}\n", pid))
}

/// POSIX: Remove PID file on exit
pub fn remove_pid_file(session_name: &str) -> io::Result<()> {
    let lock_path = lock_file_path(session_name);
    std::fs::remove_file(&lock_path)
}
