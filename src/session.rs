//! Session management: PTY + Screen + reader thread.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;

use crate::consts::{ENV_SESSION_LOG_PATH, PTY_READ_BUF_SIZE};
use crate::pmux_log;
use crate::pty::{self, PtyChild, PtyConfig, PtyMaster, PtyReader, PtyResult, WindowSize};
use crate::screen::Screen;

/// Split a Windows command-line argument string into Vec<String>.
/// Simple split on whitespace; handles double-quoted segments.
#[cfg(windows)]
fn shell_words_windows(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    for ch in s.chars() {
        match ch {
            '"' => in_quote = !in_quote,
            ' ' | '\t' if !in_quote => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Returns the session log directory if `EMX_PMUX_LOG_SESSION_PATH` is set
/// and points to an existing directory.
fn session_log_dir() -> Option<PathBuf> {
    let val = std::env::var(ENV_SESSION_LOG_PATH).ok()?;
    let path = PathBuf::from(val);
    if fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
        Some(path)
    } else {
        None
    }
}

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

            // Open session log file if EMX_PMUX_LOG_SESSION_PATH is set.
            let mut log_writer: Option<BufWriter<fs::File>> = session_log_dir().and_then(|dir| {
                let path = dir.join(format!("{}.log", name));
                match OpenOptions::new().create(true).append(true).open(&path) {
                    Ok(f) => {
                        pmux_log!("reader({}): session log → {:?}", name, path);
                        Some(BufWriter::new(f))
                    }
                    Err(e) => {
                        pmux_log!("reader({}): failed to open session log {:?}: {}", name, path, e);
                        None
                    }
                }
            });

            let mut buf = [0u8; PTY_READ_BUF_SIZE];
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
                            // Write transcript directly to log, avoiding allocation.
                            if let Some(ref mut w) = log_writer {
                                let t = scr.transcript();
                                if !t.is_empty() {
                                    let _ = w.write_all(t);
                                    let _ = w.flush();
                                }
                                scr.clear_transcript();
                            }
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        pmux_log!("reader({}): error after {} bytes: {}", name, total, e);
                        break;
                    }
                }
            }
            // Flush log before exiting.
            if let Some(ref mut w) = log_writer {
                let _ = w.flush();
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
        self.screen.lock().expect("screen mutex poisoned").text()
    }

    /// Get current screen content with ANSI/SGR attributes.
    pub fn screen_ansi(&self) -> String {
        self.screen.lock().expect("screen mutex poisoned").render_ansi()
    }

    /// Get cursor position (col, row).
    pub fn cursor_pos(&self) -> (u16, u16) {
        self.screen.lock().expect("screen mutex poisoned").cursor()
    }

    /// Get screen size.
    pub fn screen_size(&self) -> (u16, u16) {
        self.screen.lock().expect("screen mutex poisoned").size()
    }

    /// Resize PTY and screen.
    pub fn resize(&mut self, cols: u16, rows: u16) -> PtyResult<()> {
        self.master.resize(WindowSize::new(cols, rows))?;
        self.screen.lock().expect("screen mutex poisoned").resize(cols, rows);
        Ok(())
    }

    /// Check if the child process is still alive.
    ///
    /// Checks both the reader thread status (pipe EOF) and the actual
    /// child process handle.  ConPTY does not produce pipe EOF when the
    /// child exits, so we must also poll the process handle directly.
    pub fn is_alive(&self) -> bool {
        if !self.alive.load(Ordering::SeqCst) {
            return false;
        }
        self.child.is_process_alive()
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

fn session_not_found(name: &str) -> String {
    format!("Session '{}' not found", name)
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
            #[cfg(unix)]
            {
                config.args = vec!["-c".to_string(), cmd];
            }
            #[cfg(windows)]
            {
                // If the command looks like a direct executable (no spaces or
                // it's a known shell), run it directly. Otherwise wrap in
                // cmd.exe /c for compound commands.
                let trimmed = cmd.trim();
                let lower = trimmed.to_lowercase();
                if lower == "powershell.exe"
                    || lower == "powershell"
                    || lower == "pwsh.exe"
                    || lower == "pwsh"
                    || lower.starts_with("powershell.exe ")
                    || lower.starts_with("powershell ")
                    || lower.starts_with("pwsh.exe ")
                    || lower.starts_with("pwsh ")
                {
                    // Launch PowerShell directly so it gets a proper console
                    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
                    config.program = parts[0].to_string();
                    if parts.len() > 1 {
                        config.args = shell_words_windows(parts[1]);
                    }
                } else {
                    config.program = "cmd.exe".to_string();
                    config.args = vec!["/c".to_string(), cmd];
                }
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
            None => Err(session_not_found(name)),
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
            None => Err(session_not_found(name)),
        }
    }

    /// Get screen text of a session.
    pub fn view(&self, name: &str) -> Result<String, String> {
        match self.sessions.get(name) {
            Some(s) => Ok(s.screen_text()),
            None => Err(session_not_found(name)),
        }
    }

    /// Get screen content of a session with ANSI/SGR attributes.
    pub fn view_ansi(&self, name: &str) -> Result<String, String> {
        match self.sessions.get(name) {
            Some(s) => Ok(s.screen_ansi()),
            None => Err(session_not_found(name)),
        }
    }

    /// Check whether a session's child process is still alive.
    pub fn is_alive(&self, name: &str) -> Option<bool> {
        self.sessions.get(name).map(|s| s.is_alive())
    }

    /// Get cursor position of a session.
    pub fn cursor_pos(&self, name: &str) -> Result<(u16, u16), String> {
        match self.sessions.get(name) {
            Some(s) => Ok(s.cursor_pos()),
            None => Err(session_not_found(name)),
        }
    }

    /// Resize a session's PTY.
    pub fn resize(&mut self, name: &str, cols: u16, rows: u16) -> Result<(), String> {
        match self.sessions.get_mut(name) {
            Some(s) => s.resize(cols, rows).map_err(|e| format!("Resize failed: {}", e)),
            None => Err(session_not_found(name)),
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
