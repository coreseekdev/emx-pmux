//! IPC protocol for client ↔ daemon communication.
//!
//! Length-prefixed JSON messages over local sockets (interprocess crate).

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

use crate::session::SessionInfo;

// ── Request ──────────────────────────────────────────────────────────

/// Client → Daemon request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Create a new session.
    NewSession {
        name: Option<String>,
        command: Option<String>,
        cols: u16,
        rows: u16,
    },
    /// Kill a session.
    KillSession { name: String },
    /// List all sessions.
    ListSessions,
    /// Send data to a session's PTY stdin.
    SendData { name: String, data: Vec<u8> },
    /// View a session's screen content.
    ViewScreen { name: String },
    /// Resize a session's PTY.
    ResizePty { name: String, cols: u16, rows: u16 },
    /// Kill the daemon process.
    KillServer,
    /// Health check.
    Ping,
}

// ── Response ─────────────────────────────────────────────────────────

/// Daemon → Client response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Created { name: String },
    SessionList { sessions: Vec<SessionInfo> },
    Screen { content: String },
    Pong,
    Error { message: String },
}

// ── Wire format ──────────────────────────────────────────────────────
// 4 bytes big-endian length + JSON payload.

const MAX_MSG: usize = 16 * 1024 * 1024; // 16 MiB

/// Send a serializable value over a stream.
pub fn send_msg<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let json = serde_json::to_vec(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if json.len() > MAX_MSG {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let len = (json.len() as u32).to_be_bytes();
    w.write_all(&len)?;
    w.write_all(&json)?;
    w.flush()
}

/// Receive a deserializable value from a stream.
pub fn recv_msg<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> io::Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_MSG {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
