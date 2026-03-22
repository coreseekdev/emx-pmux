//! IPC protocol for client ↔ daemon communication.
//!
//! Length-prefixed JSON messages over tokio async streams.
//! The first 4 bytes of each connection are a protocol version magic (`PMUX`),
//! followed by a 2-byte protocol version (big-endian u16).

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::session::SessionInfo;

/// Protocol version. Bump when the wire format changes.
pub const PROTOCOL_VERSION: u16 = 1;

/// Magic bytes that identify a pmux IPC connection.
const MAGIC: &[u8; 4] = b"PMUX";

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
// Connection: MAGIC (4 bytes) + version (2 bytes big-endian)
// Each message: 4 bytes big-endian length + JSON payload.

const MAX_MSG: usize = 16 * 1024 * 1024; // 16 MiB

/// Write the protocol handshake header (client side).
pub async fn write_handshake<W: AsyncWrite + Unpin>(w: &mut W) -> std::io::Result<()> {
    w.write_all(MAGIC).await?;
    w.write_all(&PROTOCOL_VERSION.to_be_bytes()).await?;
    Ok(())
}

/// Read and validate the protocol handshake header (server side).
/// Returns the client's protocol version.
pub async fn read_handshake<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<u16> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).await?;
    if &magic != MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a pmux client",
        ));
    }
    let mut ver = [0u8; 2];
    r.read_exact(&mut ver).await?;
    let version = u16::from_be_bytes(ver);
    if version != PROTOCOL_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "protocol version mismatch: client={}, daemon={}",
                version, PROTOCOL_VERSION
            ),
        ));
    }
    Ok(version)
}

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
