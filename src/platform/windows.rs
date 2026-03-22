//! Windows-specific daemon implementation
//!
//! Uses Windows-specific process creation flags to spawn
//! background processes without console attachment.

use std::env;
use std::io::{self, Write};
use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use super::{DaemonConfig, DaemonHandle};

/// Windows process directory
const PMUX_DIR: &str = r"\.pmux";

/// Get the pmux directory path
pub fn pmux_dir() -> String {
    let home = env::var("USERPROFILE")
        .or_else(|_| env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    format!("{}{}", home, PMUX_DIR)
}

/// Get the port file path for a session
pub fn port_file_path(session_name: &str) -> String {
    format!("{}\\{}.port", pmux_dir(), session_name)
}

/// Get the key file path for a session
pub fn key_file_path(session_name: &str) -> String {
    format!("{}\\{}.key", pmux_dir(), session_name)
}

/// Windows-specific implementation: spawn daemon without console
pub fn spawn_daemon_impl(config: DaemonConfig) -> Result<DaemonHandle, String> {
    // Ensure directory exists
    let dir = pmux_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create pmux dir: {}", e))?;

    let exe = env::current_exe()
        .map_err(|e| format!("Failed to get executable path: {}", e))?;

    let mut cmd = Command::new(&exe);

    // Windows-specific: spawn without console window
    // DETACHED_PROCESS = 0x00000008
    // CREATE_NO_WINDOW = 0x08000000
    cmdcreation_flags(0x08000008, &mut cmd);

    // Pass daemon subcommand
    cmd.arg("daemon")
        .arg("-s")
        .arg(&config.session_name);

    // Optional: initial size for warm start
    if let Some((w, h)) = config.init_size {
        cmd.arg("-x").arg(w.to_string())
            .arg("-y").arg(h.to_string());
    }

    // Detach from parent
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Spawn the daemon process
    let child = cmd.spawn()
        .map_err(|e| format!("Failed to spawn daemon: {}", e))?;

    let pid = child.id();

    // Note: We don't wait for the child process.
    // The daemon will write its .port file when ready.
    // The caller should poll for the file to verify daemon startup.

    Ok(DaemonHandle {
        pid,
        session_name: config.session_name.clone(),
    })
}

/// Set Windows creation flags on a Command
fn cmdcreation_flags(flags: u32, cmd: &mut Command) -> &mut Command {
    cmd.creation_flags(flags);
    cmd
}

/// Windows-specific: check if daemon is running
pub fn is_daemon_running_impl(session_name: &str) -> bool {
    let port_path = port_file_path(session_name);

    // Check if port file exists
    if !std::path::Path::new(&port_path).exists() {
        return false;
    }

    // Verify daemon is actually alive by checking the port
    if let Ok(port_str) = std::fs::read_to_string(&port_path) {
        if let Ok(port) = port_str.trim().parse::<u16>() {
            let addr = format!("127.0.0.1:{}", port);
            // Try to connect with short timeout
            return std::net::TcpStream::connect_timeout(
                &addr.parse().unwrap(),
                std::time::Duration::from_millis(50),
            ).is_ok();
        }
    }

    false
}

/// Windows-specific: stop daemon by terminating process
pub fn stop_daemon_impl(session_name: &str) -> Result<(), String> {
    use std::thread;
    use std::time::Duration;

    // Try graceful shutdown via TCP first
    if let Ok(port_str) = std::fs::read_to_string(port_file_path(session_name)) {
        if let Ok(port) = port_str.trim().parse::<u16>() {
            let addr = format!("127.0.0.1:{}", port);
            if let Ok(mut stream) = std::net::TcpStream::connect_timeout(
                &addr.parse().unwrap(),
                Duration::from_millis(100),
            ) {
                // Send kill-server command (protocol to be defined)
                let _ = stream.write_all(b"kill-server\n");
                let _ = stream.flush();

                // Wait a bit for graceful shutdown
                thread::sleep(Duration::from_millis(200));

                // Check if actually stopped
                if !is_daemon_running_impl(session_name) {
                    // Clean up files
                    let _ = std::fs::remove_file(port_file_path(session_name));
                    let _ = std::fs::remove_file(key_file_path(session_name));
                    return Ok(());
                }
            }
        }
    }

    // Fallback: Find and terminate process by command line
    terminate_process_by_session(session_name)
}

/// Terminate a daemon process by session name (Windows fallback)
fn terminate_process_by_session(session_name: &str) -> Result<(), String> {
    // Use PowerShell to find and kill the process
    let ps_script = format!(
        "Get-Process pmux | Where-Object {{ $_.Path -eq (Get-Command pmux).Source }} | \
         Where-Object {{ $_.CommandLine -like '*-s {}*' }} | \
         Stop-Process -Force",
        session_name
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps_script])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();

    match output {
        Ok(_) => {
            // Clean up files
            let _ = std::fs::remove_file(port_file_path(session_name));
            let _ = std::fs::remove_file(key_file_path(session_name));
            Ok(())
        }
        Err(e) => Err(format!("Failed to terminate daemon: {}", e)),
    }
}

/// Windows-specific: list running daemons
pub fn list_daemons_impl() -> Vec<String> {
    let mut daemons = Vec::new();
    let dir = pmux_dir();

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "port").unwrap_or(false) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    // Verify it's actually running
                    if is_daemon_running_impl(stem) {
                        daemons.push(stem.to_string());
                    } else {
                        // Clean up stale port file
                        let _ = std::fs::remove_file(&path);
                        let key_path = path.with_extension("key");
                        let _ = std::fs::remove_file(&key_path);
                    }
                }
            }
        }
    }

    daemons.sort();
    daemons
}

/// Windows: Spawn process completely hidden (for warm server)
pub fn spawn_hidden(args: &[String]) -> io::Result<std::process::Child> {
    let exe = env::current_exe()?;
    let mut cmd = Command::new(&exe);
    cmdcreation_flags(0x08000008, &mut cmd); // CREATE_NO_WINDOW
    cmd.args(args);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}
