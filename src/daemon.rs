//! Daemon management
//!
//! Handles daemon lifecycle: start, stop, status, and auto-spawn on demand.

use crate::platform::{self, DaemonConfig, DaemonHandle};

/// Daemon manager
pub struct DaemonManager {
    /// Session namespace (-L flag)
    pub namespace: Option<String>,
}

impl DaemonManager {
    /// Create a new daemon manager
    pub fn new() -> Self {
        DaemonManager {
            namespace: None,
        }
    }

    /// Set namespace for session isolation
    pub fn with_namespace(mut self, namespace: String) -> Self {
        self.namespace = Some(namespace);
        self
    }

    /// Get the full session name (with namespace prefix)
    pub fn full_session_name(&self, session_name: &str) -> String {
        if let Some(ref ns) = self.namespace {
            format!("{}__{}", ns, session_name)
        } else {
            session_name.to_string()
        }
    }

    /// Start a daemon explicitly (pmux start command)
    pub fn start(&self, session_name: &str) -> Result<DaemonHandle, String> {
        let full_name = self.full_session_name(session_name);

        // Check if already running
        if platform::is_daemon_running(&full_name) {
            return Err(format!("Daemon '{}' is already running", session_name));
        }

        let config = DaemonConfig {
            session_name: full_name.clone(),
            work_dir: None,
            env: Vec::new(),
            init_size: None,
        };

        platform::spawn_daemon(config)
    }

    /// Stop a daemon (pmux stop command)
    pub fn stop(&self, session_name: &str) -> Result<(), String> {
        let full_name = self.full_session_name(session_name);

        if !platform::is_daemon_running(&full_name) {
            return Err(format!("Daemon '{}' is not running", session_name));
        }

        platform::stop_daemon(&full_name)
    }

    /// Get daemon status
    pub fn status(&self, session_name: &str) -> DaemonStatus {
        let full_name = self.full_session_name(session_name);

        if platform::is_daemon_running(&full_name) {
            DaemonStatus::Running
        } else {
            DaemonStatus::Stopped
        }
    }

    /// List all running daemons
    pub fn list(&self) -> Vec<String> {
        let all = platform::list_daemons();

        // Filter by namespace if set
        if let Some(ref ns) = self.namespace {
            let prefix = format!("{}__", ns);
            all.into_iter()
                .filter(|name| name.starts_with(&prefix))
                .map(|name| name[prefix.len()..].to_string())
                .collect()
        } else {
            // Exclude namespaced sessions
            all.into_iter()
                .filter(|name| !name.contains("__"))
                .collect()
        }
    }

    /// Ensure daemon is running (auto-spawn if needed)
    ///
    /// This is used by commands like `new` or `attach` to automatically
    /// start a daemon if one doesn't exist.
    pub fn ensure(&self, session_name: &str) -> Result<DaemonHandle, String> {
        let full_name = self.full_session_name(session_name);

        if platform::is_daemon_running(&full_name) {
            // Already running, return a dummy handle
            return Ok(DaemonHandle {
                pid: 0,  // We don't know the actual PID
                session_name: full_name,
            });
        }

        // Auto-spawn daemon
        let config = DaemonConfig {
            session_name: full_name.clone(),
            work_dir: None,
            env: Vec::new(),
            init_size: None,
        };

        platform::spawn_daemon(config)
    }

    /// Generate next available session name
    pub fn next_session_name(&self) -> String {
        let existing = self.list();

        // Find the lowest non-negative integer not in use
        let mut id = 0u32;
        while existing.iter().any(|name| {
            name.parse::<u32>().ok() == Some(id)
        }) {
            id += 1;
        }

        id.to_string()
    }
}

impl Default for DaemonManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Daemon status
#[derive(Debug, Clone, PartialEq)]
pub enum DaemonStatus {
    Running,
    Stopped,
}

/// Wait for daemon to be ready (polling for .port/.sock file)
pub fn wait_for_daemon(session_name: &str, timeout_ms: u64) -> Result<(), String> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);

    while start.elapsed() < timeout {
        if cfg!(windows) {
            // Check for .port file
            if let Ok(port_path) = std::fs::read_to_string(format!("{}/{}.port", platform::pmux_dir(), session_name)) {
                // Verify daemon is actually accepting connections
                if platform::is_daemon_running(session_name) {
                    return Ok(());
                }
            }
        } else {
            // Check for .sock file (POSIX)
            if let Ok(_) = std::fs::metadata(format!("{}/{}.sock", platform::pmux_dir(), session_name)) {
                return Ok(());
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    Err(format!("Daemon '{}' failed to start within {}ms", session_name, timeout_ms))
}
