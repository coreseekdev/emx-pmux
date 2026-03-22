//! IPC protocol for client ↔ daemon communication.
//!
//! Binary-framed, typed messages inspired by tmux's imsgbuf protocol.
//!
//! Wire format:
//!   Handshake (once per connection): PMUX (4 bytes) + version (u16 BE)
//!   Each message: type (u16 BE) + payload_len (u32 BE) + payload
//!
//! Structured payloads use JSON; data-heavy messages use binary encoding.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::session::SessionInfo;

/// Protocol version. Bump when the wire format changes.
pub const PROTOCOL_VERSION: u16 = 2;

/// Magic bytes that identify a pmux IPC connection.
const MAGIC: &[u8; 4] = b"PMUX";

const MAX_MSG: usize = 16 * 1024 * 1024; // 16 MiB

// ── Message type IDs (tmux-style numbered types) ─────────────────────

// Client → Daemon
const MSG_NEW_SESSION: u16 = 0x0001;
const MSG_KILL_SESSION: u16 = 0x0002;
const MSG_LIST_SESSIONS: u16 = 0x0003;
const MSG_SEND_DATA: u16 = 0x0004;
const MSG_VIEW_SCREEN: u16 = 0x0005;
const MSG_RESIZE_PTY: u16 = 0x0006;
const MSG_KILL_SERVER: u16 = 0x0007;
const MSG_PING: u16 = 0x0008;
// Reserved for future: ATTACH(0x0009), DETACH(0x000A), EXPECT(0x000B), READ(0x000C)

// Daemon → Client
const MSG_OK: u16 = 0x0100;
const MSG_ERROR: u16 = 0x0101;
const MSG_CREATED: u16 = 0x0102;
const MSG_SESSION_LIST: u16 = 0x0103;
const MSG_SCREEN_DATA: u16 = 0x0104;
const MSG_PONG: u16 = 0x0105;
// Reserved for future: DATA(0x0106), MATCHED(0x0107), EOF(0x0108)

// ── Message ──────────────────────────────────────────────────────────

/// Unified message type for bidirectional client ↔ daemon communication.
#[derive(Debug, Clone)]
pub enum Message {
    // Client → Daemon
    NewSession {
        name: Option<String>,
        command: Option<String>,
        cols: u16,
        rows: u16,
    },
    KillSession { name: String },
    ListSessions,
    /// Send raw data to a session's PTY stdin (binary-encoded on the wire).
    SendData { name: String, data: Vec<u8> },
    ViewScreen { name: String },
    ResizePty { name: String, cols: u16, rows: u16 },
    KillServer,
    Ping,

    // Daemon → Client
    Ok,
    Error { message: String },
    Created { name: String },
    SessionList { sessions: Vec<SessionInfo> },
    /// Screen content returned as raw UTF-8 (no JSON wrapping).
    ScreenData { content: String },
    Pong,
}

// ── JSON helper structs for structured payloads ──────────────────────

#[derive(Serialize, Deserialize)]
struct NewSessionPayload {
    name: Option<String>,
    command: Option<String>,
    cols: u16,
    rows: u16,
}

#[derive(Serialize, Deserialize)]
struct NamePayload {
    name: String,
}

#[derive(Serialize, Deserialize)]
struct ResizePayload {
    name: String,
    cols: u16,
    rows: u16,
}

#[derive(Serialize, Deserialize)]
struct ErrorPayload {
    message: String,
}

#[derive(Serialize, Deserialize)]
struct SessionListPayload {
    sessions: Vec<SessionInfo>,
}

// ── Handshake ────────────────────────────────────────────────────────

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

// ── Wire format: write / read ────────────────────────────────────────

/// Write a message to an async stream (type + length + payload).
pub async fn write_msg<W: AsyncWrite + Unpin>(
    w: &mut W,
    msg: &Message,
) -> std::io::Result<()> {
    let (msg_type, payload) = encode(msg)?;
    if payload.len() > MAX_MSG {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    w.write_all(&msg_type.to_be_bytes()).await?;
    w.write_all(&(payload.len() as u32).to_be_bytes()).await?;
    w.write_all(&payload).await?;
    w.flush().await
}

/// Read a message from an async stream (type + length + payload).
pub async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Message> {
    let mut hdr = [0u8; 6];
    r.read_exact(&mut hdr).await?;
    let msg_type = u16::from_be_bytes([hdr[0], hdr[1]]);
    let payload_len = u32::from_be_bytes([hdr[2], hdr[3], hdr[4], hdr[5]]) as usize;
    if payload_len > MAX_MSG {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        r.read_exact(&mut payload).await?;
    }
    decode(msg_type, &payload)
}

// ── Encoding ─────────────────────────────────────────────────────────

fn json_io_err(e: serde_json::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e)
}

fn encode(msg: &Message) -> std::io::Result<(u16, Vec<u8>)> {
    match msg {
        Message::NewSession { name, command, cols, rows } => {
            let p = NewSessionPayload {
                name: name.clone(),
                command: command.clone(),
                cols: *cols,
                rows: *rows,
            };
            Ok((MSG_NEW_SESSION, serde_json::to_vec(&p).map_err(json_io_err)?))
        }
        Message::KillSession { name } => {
            Ok((MSG_KILL_SESSION, serde_json::to_vec(&NamePayload { name: name.clone() }).map_err(json_io_err)?))
        }
        Message::ListSessions => Ok((MSG_LIST_SESSIONS, Vec::new())),
        Message::SendData { name, data } => {
            // Binary: name_len(u16 BE) + name(UTF-8) + raw data
            let nb = name.as_bytes();
            let mut payload = Vec::with_capacity(2 + nb.len() + data.len());
            payload.extend_from_slice(&(nb.len() as u16).to_be_bytes());
            payload.extend_from_slice(nb);
            payload.extend_from_slice(data);
            Ok((MSG_SEND_DATA, payload))
        }
        Message::ViewScreen { name } => {
            Ok((MSG_VIEW_SCREEN, serde_json::to_vec(&NamePayload { name: name.clone() }).map_err(json_io_err)?))
        }
        Message::ResizePty { name, cols, rows } => {
            let p = ResizePayload { name: name.clone(), cols: *cols, rows: *rows };
            Ok((MSG_RESIZE_PTY, serde_json::to_vec(&p).map_err(json_io_err)?))
        }
        Message::KillServer => Ok((MSG_KILL_SERVER, Vec::new())),
        Message::Ping => Ok((MSG_PING, Vec::new())),

        // Daemon → Client
        Message::Ok => Ok((MSG_OK, Vec::new())),
        Message::Error { message } => {
            Ok((MSG_ERROR, serde_json::to_vec(&ErrorPayload { message: message.clone() }).map_err(json_io_err)?))
        }
        Message::Created { name } => {
            Ok((MSG_CREATED, serde_json::to_vec(&NamePayload { name: name.clone() }).map_err(json_io_err)?))
        }
        Message::SessionList { sessions } => {
            Ok((MSG_SESSION_LIST, serde_json::to_vec(&SessionListPayload { sessions: sessions.clone() }).map_err(json_io_err)?))
        }
        Message::ScreenData { content } => {
            // Raw UTF-8 bytes — no JSON wrapping overhead
            Ok((MSG_SCREEN_DATA, content.as_bytes().to_vec()))
        }
        Message::Pong => Ok((MSG_PONG, Vec::new())),
    }
}

// ── Decoding ─────────────────────────────────────────────────────────

fn proto_err(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

fn decode(msg_type: u16, payload: &[u8]) -> std::io::Result<Message> {
    match msg_type {
        MSG_NEW_SESSION => {
            let p: NewSessionPayload = serde_json::from_slice(payload).map_err(json_io_err)?;
            Ok(Message::NewSession { name: p.name, command: p.command, cols: p.cols, rows: p.rows })
        }
        MSG_KILL_SESSION => {
            let p: NamePayload = serde_json::from_slice(payload).map_err(json_io_err)?;
            Ok(Message::KillSession { name: p.name })
        }
        MSG_LIST_SESSIONS => Ok(Message::ListSessions),
        MSG_SEND_DATA => {
            // Binary: name_len(u16 BE) + name(UTF-8) + raw data
            if payload.len() < 2 {
                return Err(proto_err("SendData payload too short"));
            }
            let name_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
            if payload.len() < 2 + name_len {
                return Err(proto_err("SendData name truncated"));
            }
            let name = std::str::from_utf8(&payload[2..2 + name_len])
                .map_err(|_| proto_err("invalid UTF-8 in session name"))?
                .to_string();
            let data = payload[2 + name_len..].to_vec();
            Ok(Message::SendData { name, data })
        }
        MSG_VIEW_SCREEN => {
            let p: NamePayload = serde_json::from_slice(payload).map_err(json_io_err)?;
            Ok(Message::ViewScreen { name: p.name })
        }
        MSG_RESIZE_PTY => {
            let p: ResizePayload = serde_json::from_slice(payload).map_err(json_io_err)?;
            Ok(Message::ResizePty { name: p.name, cols: p.cols, rows: p.rows })
        }
        MSG_KILL_SERVER => Ok(Message::KillServer),
        MSG_PING => Ok(Message::Ping),

        MSG_OK => Ok(Message::Ok),
        MSG_ERROR => {
            let p: ErrorPayload = serde_json::from_slice(payload).map_err(json_io_err)?;
            Ok(Message::Error { message: p.message })
        }
        MSG_CREATED => {
            let p: NamePayload = serde_json::from_slice(payload).map_err(json_io_err)?;
            Ok(Message::Created { name: p.name })
        }
        MSG_SESSION_LIST => {
            let p: SessionListPayload = serde_json::from_slice(payload).map_err(json_io_err)?;
            Ok(Message::SessionList { sessions: p.sessions })
        }
        MSG_SCREEN_DATA => {
            let content = std::str::from_utf8(payload)
                .map_err(|_| proto_err("invalid UTF-8 in screen data"))?
                .to_string();
            Ok(Message::ScreenData { content })
        }
        MSG_PONG => Ok(Message::Pong),

        _ => Err(proto_err(&format!("unknown message type: 0x{:04x}", msg_type))),
    }
}
