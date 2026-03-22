//! Session management: PTY + Screen + reader thread.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;

use crate::pty::{self, PtyChild, PtyConfig, PtyMaster, PtyResult, WindowSize};
use crate::screen::Screen;

/// A single session: owns a PTY and a screen buffer.
pub struct Session {
    pub name: String,
    pub command: String,
    pub created_at: u64,
    master: PtyMaster,
    child: PtyChild,
    screen: Arc<Mutex<Screen>>,
    /// Flag: reader thread should stop when child exits.
    alive: Arc<Mutex<bool>>,
}

impl Session {
    /// Spawn a new session with the given name and config.
    pub fn spawn(name: String, config: PtyConfig) -> PtyResult<Self> {
        let command = config.program.clone();
        let size = config.size;
        let (master, child) = pty::spawn(&config)?;

        let screen = Arc::new(Mutex::new(Screen::new(size.cols, size.rows)));
        let alive = Arc::new(Mutex::new(true));

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
    fn start_reader(&mut self) {
        let screen = Arc::clone(&self.screen);
        let alive = Arc::clone(&self.alive);

        // Clone the master fd for reading in the background thread.
        // On both platforms PtyMaster can be try_clone'd (via OwnedFd::try_clone / DuplicateHandle).
        let mut reader = match self.master.try_clone() {
            Ok(r) => r,
            Err(_) => return,
        };

        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut scr) = screen.lock() {
                            scr.feed(&buf[..n]);
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            if let Ok(mut a) = alive.lock() {
                *a = false;
            }
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
        *self.alive.lock().unwrap()
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

/// Serializable session info for IPC responses.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
            let n = self.next_id.to_string();
            self.next_id += 1;
            // Ensure unique
            if self.sessions.contains_key(&n) {
                loop {
                    let candidate = self.next_id.to_string();
                    self.next_id += 1;
                    if !self.sessions.contains_key(&candidate) {
                        return candidate;
                    }
                }
            }
            n
        });

        if self.sessions.contains_key(&name) {
            return Err(format!("Session '{}' already exists", name));
        }

        let mut config = PtyConfig::default();
        if let Some(cmd) = command {
            config.program = cmd;
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
