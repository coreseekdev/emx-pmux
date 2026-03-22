//! Session management: PTY + Screen + reader thread.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;

use crate::pmux_log;
use crate::pty::{self, PtyChild, PtyConfig, PtyMaster, PtyReader, PtyResult, WindowSize};
use crate::screen::Screen;

/// A single session: owns a PTY and a screen buffer.
pub struct Session {
    pub name: String,
    pub command: String,
    pub created_at: u64,
    master: PtyMaster,
    child: PtyChild,
    screen: Arc<Mutex<Screen>>,
    alive: Arc<AtomicBool>,
}

impl Session {
    /// Spawn a new session with the given name and config.
    pub fn spawn(name: String, config: PtyConfig) -> PtyResult<Self> {
        let command = config.program.clone();
        let size = config.size;
        let (master, child) = pty::spawn(&config)?;

        let screen = Arc::new(Mutex::new(Screen::new(size.cols, size.rows)));
        let alive = Arc::new(AtomicBool::new(true));

        let created_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut session = Session {
            name,
            command,
            created_at,
            master,
            child,
            screen: Arc::clone(&screen),
            alive: Arc::clone(&alive),
        };

        session.start_reader();
        Ok(session)
    }

    /// Start background reader thread that feeds PTY output into Screen.
    ///
    /// PTY reads are inherently blocking I/O (file descriptor / HANDLE),
    /// so we keep a real OS thread here rather than a tokio task.
    fn start_reader(&mut self) {
        let screen = Arc::clone(&self.screen);
        let alive = Arc::clone(&self.alive);

        let mut reader: PtyReader = match self.master.try_clone() {
            Ok(r) => r,
            Err(e) => {
                pmux_log!("reader: try_clone failed: {}", e);
                return;
            }
        };

        let name = self.name.clone();
        thread::spawn(move || {
            pmux_log!("reader({}): started", name);
            let mut buf = [0u8; 8192];
            let mut total = 0usize;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        pmux_log!("reader({}): EOF after {} bytes", name, total);
                        break;
                    }
                    Ok(n) => {
                        total += n;
                        if total <= 256 {
                            pmux_log!("reader({}): read {} bytes (total {}), data={:02x?}", name, n, total, &buf[..n.min(64)]);
                        } else {
                            pmux_log!("reader({}): read {} bytes (total {})", name, n, total);
                        }
                        if let Ok(mut scr) = screen.lock() {
                            scr.feed(&buf[..n]);
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        pmux_log!("reader({}): error after {} bytes: {}", name, total, e);
                        break;
                    }
                }
            }
            alive.store(false, Ordering::SeqCst);
        });
    }

    /// Write data to the PTY (e.g. user keystrokes).
    pub fn write_data(&mut self, data: &[u8]) -> io::Result<()> {
        self.master.write_all(data)
    }

    /// Get current screen content as text.
    pub fn screen_text(&self) -> String {
        self.screen.lock().unwrap().text()
    }

    /// Get cursor position (col, row).
    pub fn cursor_pos(&self) -> (u16, u16) {
        self.screen.lock().unwrap().cursor()
    }

    /// Get screen size.
    pub fn screen_size(&self) -> (u16, u16) {
        self.screen.lock().unwrap().size()
    }

    /// Resize PTY and screen.
    pub fn resize(&mut self, cols: u16, rows: u16) -> PtyResult<()> {
        self.master.resize(WindowSize::new(cols, rows))?;
        self.screen.lock().unwrap().resize(cols, rows);
        Ok(())
    }

    /// Check if the child process is still alive.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Kill the child process.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// Get session info for listing.
    pub fn info(&self) -> SessionInfo {
        SessionInfo {
            name: self.name.clone(),
            command: self.command.clone(),
            created_at: self.created_at,
            alive: self.is_alive(),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Session info for IPC responses.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub name: String,
    pub command: String,
    pub created_at: u64,
    pub alive: bool,
}

/// Manages all sessions in the daemon.
pub struct SessionManager {
    sessions: HashMap<String, Session>,
    next_id: u32,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            next_id: 0,
        }
    }

    /// Create a new session. Returns the assigned name.
    pub fn create(
        &mut self,
        name: Option<String>,
        command: Option<String>,
        cols: u16,
        rows: u16,
    ) -> Result<String, String> {
        let name = name.unwrap_or_else(|| {
            loop {
                let candidate = self.next_id.to_string();
                self.next_id += 1;
                if !self.sessions.contains_key(&candidate) {
                    return candidate;
                }
            }
        });

        if self.sessions.contains_key(&name) {
            return Err(format!("Session '{}' already exists", name));
        }

        let mut config = PtyConfig::default();
        if let Some(cmd) = command {
            // Wrap the command in a shell so compound commands work
            // (e.g. "tail -f /var/log/syslog" or "echo hello && sleep 1")
            #[cfg(unix)]
            {
                config.args = vec!["-c".to_string(), cmd];
                // Keep config.program as default shell ($SHELL or /bin/sh)
            }
            #[cfg(windows)]
            {
                config.program = "cmd.exe".to_string();
                config.args = vec!["/c".to_string(), cmd];
            }
        }
        config.size = WindowSize::new(cols, rows);

        let session = Session::spawn(name.clone(), config)
            .map_err(|e| format!("Failed to spawn session: {}", e))?;

        self.sessions.insert(name.clone(), session);
        Ok(name)
    }

    /// Kill a session by name.
    pub fn kill(&mut self, name: &str) -> Result<(), String> {
        match self.sessions.remove(name) {
            Some(mut s) => {
                s.kill();
                Ok(())
            }
            None => Err(format!("Session '{}' not found", name)),
        }
    }

    /// List all sessions.
    pub fn list(&self) -> Vec<SessionInfo> {
        self.sessions.values().map(|s| s.info()).collect()
    }

    /// Send data to a session's PTY.
    pub fn send(&mut self, name: &str, data: &[u8]) -> Result<(), String> {
        match self.sessions.get_mut(name) {
            Some(s) => s
                .write_data(data)
                .map_err(|e| format!("Write failed: {}", e)),
            None => Err(format!("Session '{}' not found", name)),
        }
    }

    /// Get screen text of a session.
    pub fn view(&self, name: &str) -> Result<String, String> {
        match self.sessions.get(name) {
            Some(s) => Ok(s.screen_text()),
            None => Err(format!("Session '{}' not found", name)),
        }
    }

    /// Get cursor position of a session.
    pub fn cursor_pos(&self, name: &str) -> Result<(u16, u16), String> {
        match self.sessions.get(name) {
            Some(s) => Ok(s.cursor_pos()),
            None => Err(format!("Session '{}' not found", name)),
        }
    }

    /// Resize a session's PTY.
    pub fn resize(&mut self, name: &str, cols: u16, rows: u16) -> Result<(), String> {
        match self.sessions.get_mut(name) {
            Some(s) => s.resize(cols, rows).map_err(|e| format!("Resize failed: {}", e)),
            None => Err(format!("Session '{}' not found", name)),
        }
    }

    /// Number of active sessions.
    pub fn count(&self) -> usize {
        self.sessions.len()
    }

    /// Reap dead sessions. Returns names of removed sessions.
    pub fn reap_dead(&mut self) -> Vec<String> {
        let dead: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| !s.is_alive())
            .map(|(n, _)| n.clone())
            .collect();
        for name in &dead {
            self.sessions.remove(name);
        }
        dead
    }
}
