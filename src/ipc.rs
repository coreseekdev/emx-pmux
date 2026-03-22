//! IPC protocol for client ↔ daemon communication.
//!
//! Length-prefixed JSON messages over tokio async streams.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

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

/// Send a serializable value over an async stream.
pub async fn send_msg<W: AsyncWrite + Unpin, T: Serialize>(
    w: &mut W,
    msg: &T,
) -> std::io::Result<()> {
    let json = serde_json::to_vec(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if json.len() > MAX_MSG {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let len = (json.len() as u32).to_be_bytes();
    w.write_all(&len).await?;
    w.write_all(&json).await?;
    w.flush().await
}

/// Receive a deserializable value from an async stream.
pub async fn recv_msg<R: AsyncRead + Unpin, T: for<'de> Deserialize<'de>>(
    r: &mut R,
) -> std::io::Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_MSG {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
