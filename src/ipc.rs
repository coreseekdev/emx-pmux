//! IPC protocol for client ↔ daemon communication.
//!
//! Modelled after GNU Screen's socket protocol with tmux-style extensions.
//!
//! ## Design rationale
//!
//! **Screen model (primary)**:
//! - Fixed-size `Message` struct sent atomically per connection
//! - `protocol_revision` magic (4 bytes) + `type` (i32) as header
//! - `MSG_REVISION = ('m'<<24)|('s'<<16)|('g'<<8)|VERSION` for magic
//! - Union payload with type-specific layouts
//! - One message per connection (connect → write → close)
//!
//! **tmux additions**:
//! - Numbered MSG_* type IDs with reserved ranges for extensibility
//! - Length-prefixed variable payload (for data that exceeds fixed buffers)
//!
//! ## Wire format
//!
//! ```text
//! ┌─────────────────────┬───────────┬────────────┬─────────────┐
//! │ protocol_revision   │ type      │ payload_len│ payload     │
//! │ (4 bytes, Screen    │ (i32 LE)  │ (u32 LE)   │ (variable)  │
//! │  magic compatible)  │           │            │             │
//! └─────────────────────┴───────────┴────────────┴─────────────┘
//! ```
//!
//! - Header: 12 bytes (revision + type + payload_len)
//! - Payload: type-dependent binary layout (NOT JSON)
//! - Little-endian throughout (matches Screen's native int on x86)

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::session::SessionInfo;

// ── Protocol revision (Screen-compatible magic) ──────────────────────

/// Protocol structure version. Bump when wire layout changes.
///
/// Screen uses `MSG_VERSION = 4` with magic `('m'<<24)|('s'<<16)|('g'<<8)|4`.
/// We use the same formula with our own version counter so clients can
/// detect incompatible daemons without ambiguity.
const MSG_VERSION: u8 = 1;

/// Screen-compatible revision magic.
/// `('p'<<24) | ('m'<<16) | ('x'<<8) | MSG_VERSION`
/// = 0x706d_7801 (ASCII "pmx\x01")
pub const PROTOCOL_REVISION: i32 =
    (b'p' as i32) << 24 | (b'm' as i32) << 16 | (b'x' as i32) << 8 | MSG_VERSION as i32;

use crate::consts::MAX_IPC_MESSAGE_SIZE;

const MAX_MSG: usize = MAX_IPC_MESSAGE_SIZE;

// ── Message types (Screen-like numbering) ────────────────────────────
//
// Screen uses:
//   MSG_CREATE=0, MSG_ERROR=1, MSG_ATTACH=2, MSG_CONT=3, MSG_DETACH=4,
//   MSG_POW_DETACH=5, MSG_WINCH=6, MSG_HANGUP=7, MSG_COMMAND=8, MSG_QUERY=9
//
// We keep compatible values where semantics overlap, and extend with
// our own types in a higher range (≥100, tmux-style).

// Client → Daemon (Screen-compatible range 0–9)
pub const MSG_CREATE: i32 = 0;       // Screen MSG_CREATE — create new session
pub const MSG_ERROR: i32 = 1;        // Screen MSG_ERROR  — error message
pub const MSG_ATTACH: i32 = 2;       // Screen MSG_ATTACH — attach to session (reserved)
pub const MSG_CONT: i32 = 3;         // Screen MSG_CONT   — continue (reserved)
pub const MSG_DETACH: i32 = 4;       // Screen MSG_DETACH — detach (reserved)
pub const MSG_POW_DETACH: i32 = 5;   // Screen MSG_POW_DETACH (reserved)
pub const MSG_WINCH: i32 = 6;        // Screen MSG_WINCH  — window size changed
pub const MSG_HANGUP: i32 = 7;       // Screen MSG_HANGUP — hangup/kill session
pub const MSG_COMMAND: i32 = 8;       // Screen MSG_COMMAND — execute command
pub const MSG_QUERY: i32 = 9;         // Screen MSG_QUERY  — query (reserved)

// Client → Daemon (pmux extensions, ≥100)
pub const MSG_SEND_DATA: i32 = 100;   // Send raw data to PTY stdin
pub const MSG_VIEW_SCREEN: i32 = 101; // Request screen buffer content
pub const MSG_RESIZE_PTY: i32 = 102;  // Resize PTY
pub const MSG_KILL_SERVER: i32 = 103; // Shutdown daemon
pub const MSG_PING: i32 = 104;        // Health check
pub const MSG_LIST_SESSIONS: i32 = 105; // List all sessions

// Daemon → Client (responses, ≥200, tmux-style)
pub const MSG_OK: i32 = 200;
pub const MSG_CREATED: i32 = 201;
pub const MSG_SESSION_LIST: i32 = 202;
pub const MSG_SCREEN_DATA: i32 = 203;
pub const MSG_PONG: i32 = 204;

// Reserved for future expect-takeover API (≥300)
// pub const MSG_EXPECT: i32 = 300;
// pub const MSG_READ: i32 = 301;
// pub const MSG_MATCHED: i32 = 302;
// pub const MSG_EOF: i32 = 303;

// ── Message ──────────────────────────────────────────────────────────

/// Unified message type for bidirectional communication.
///
/// On the wire each variant maps to a MSG_* type with a binary payload.
/// Fixed-size fields use Screen-style null-terminated strings;
/// variable-length data uses raw bytes.
#[derive(Debug, Clone)]
pub enum Message {
    // -- Screen-compatible types --

    /// MSG_CREATE: Create a new session.
    /// Screen packs args in `m.m.create.line` (null-separated), dir, screenterm.
    /// We carry: name (optional), command (optional), cols, rows.
    Create {
        name: Option<String>,
        command: Option<String>,
        cols: u16,
        rows: u16,
    },

    /// MSG_ERROR: Error response with human-readable message.
    /// Screen: `m.m.message[MAXPATHLEN*2]`.
    Error { message: String },

    /// MSG_WINCH: Terminal size changed (resize PTY).
    /// Screen: display calls this on SIGWINCH.
    Winch { name: String, cols: u16, rows: u16 },

    /// MSG_HANGUP: Kill a session.
    /// Screen: sent on SIGHUP or forced disconnect.
    Hangup { name: String },

    /// MSG_COMMAND: Execute a command on a session.
    /// Screen packs args in `m.m.command.cmd` (null-separated) with `nargs`.
    /// We encode: session name + arg count + null-separated args.
    Command {
        name: String,
        args: Vec<String>,
    },

    // -- pmux extension types --

    /// MSG_SEND_DATA: Send raw bytes to a session's PTY stdin.
    SendData { name: String, data: Vec<u8> },

    /// MSG_VIEW_SCREEN: Request current screen buffer content.
    ViewScreen { name: String },

    /// MSG_RESIZE_PTY: Resize a session's PTY (named variant of WINCH).
    ResizePty { name: String, cols: u16, rows: u16 },

    /// MSG_KILL_SERVER: Shutdown the daemon process.
    KillServer,

    /// MSG_PING: Health check.
    Ping,

    /// MSG_LIST_SESSIONS: List all sessions.
    ListSessions,

    // -- Response types --

    /// MSG_OK: Success with no payload.
    Ok,

    /// MSG_CREATED: Session created, returns assigned name.
    Created { name: String },

    /// MSG_SESSION_LIST: List of session info.
    SessionList { sessions: Vec<SessionInfo> },

    /// MSG_SCREEN_DATA: Screen buffer content with cursor position.
    ScreenData { content: String, cursor_col: u16, cursor_row: u16 },

    /// MSG_PONG: Reply to Ping.
    Pong,
}

// ── Wire header ──────────────────────────────────────────────────────
//
// 12 bytes total, matching Screen's Message header layout concept:
//   protocol_revision: i32 LE (4 bytes)
//   type:              i32 LE (4 bytes)
//   payload_len:       u32 LE (4 bytes)

const HEADER_SIZE: usize = 12;

/// Write a message to an async stream.
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
    // Write 12-byte header
    w.write_all(&PROTOCOL_REVISION.to_le_bytes()).await?;
    w.write_all(&msg_type.to_le_bytes()).await?;
    w.write_all(&(payload.len() as u32).to_le_bytes()).await?;
    // Write payload
    if !payload.is_empty() {
        w.write_all(&payload).await?;
    }
    w.flush().await
}

/// Read a message from an async stream.
pub async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Message> {
    let mut hdr = [0u8; HEADER_SIZE];
    r.read_exact(&mut hdr).await?;

    let revision = i32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    if revision != PROTOCOL_REVISION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "invalid message (magic 0x{:08x}, expected 0x{:08x})",
                revision, PROTOCOL_REVISION
            ),
        ));
    }

    let msg_type = i32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
    let payload_len = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;

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

// ── Binary payload encoding ──────────────────────────────────────────
//
// Screen uses fixed C structs with null-terminated char arrays.
// We use variable-length binary encoding for efficiency, but follow
// Screen's patterns:
//   - Strings: u16-LE length prefix + UTF-8 bytes (no null terminator)
//   - Args list: u16-LE count + (u16-LE len + bytes)*
//   - Raw data: prefixed by session name, then raw bytes
//   - Integers: u16-LE

fn encode(msg: &Message) -> std::io::Result<(i32, Vec<u8>)> {
    match msg {
        Message::Create { name, command, cols, rows } => {
            // cols(u16) + rows(u16) + name(opt str) + command(opt str)
            let mut p = Vec::new();
            p.extend_from_slice(&cols.to_le_bytes());
            p.extend_from_slice(&rows.to_le_bytes());
            write_opt_str(&mut p, name.as_deref());
            write_opt_str(&mut p, command.as_deref());
            Ok((MSG_CREATE, p))
        }
        Message::Error { message } => {
            Ok((MSG_ERROR, message.as_bytes().to_vec()))
        }
        Message::Winch { name, cols, rows } => {
            let mut p = Vec::new();
            write_str(&mut p, name);
            p.extend_from_slice(&cols.to_le_bytes());
            p.extend_from_slice(&rows.to_le_bytes());
            Ok((MSG_WINCH, p))
        }
        Message::Hangup { name } => {
            Ok((MSG_HANGUP, name.as_bytes().to_vec()))
        }
        Message::Command { name, args } => {
            // Screen: nargs(i32) + null-separated args in cmd[]
            // We: name(str) + nargs(u16) + args(str*)
            let mut p = Vec::new();
            write_str(&mut p, name);
            p.extend_from_slice(&(args.len() as u16).to_le_bytes());
            for arg in args {
                write_str(&mut p, arg);
            }
            Ok((MSG_COMMAND, p))
        }
        Message::SendData { name, data } => {
            // name(str) + raw data bytes
            let mut p = Vec::with_capacity(2 + name.len() + data.len());
            write_str(&mut p, name);
            p.extend_from_slice(data);
            Ok((MSG_SEND_DATA, p))
        }
        Message::ViewScreen { name } => {
            Ok((MSG_VIEW_SCREEN, name.as_bytes().to_vec()))
        }
        Message::ResizePty { name, cols, rows } => {
            let mut p = Vec::new();
            write_str(&mut p, name);
            p.extend_from_slice(&cols.to_le_bytes());
            p.extend_from_slice(&rows.to_le_bytes());
            Ok((MSG_RESIZE_PTY, p))
        }
        Message::KillServer => Ok((MSG_KILL_SERVER, Vec::new())),
        Message::Ping => Ok((MSG_PING, Vec::new())),
        Message::ListSessions => Ok((MSG_LIST_SESSIONS, Vec::new())),

        // Responses
        Message::Ok => Ok((MSG_OK, Vec::new())),
        Message::Created { name } => {
            Ok((MSG_CREATED, name.as_bytes().to_vec()))
        }
        Message::SessionList { sessions } => {
            // Binary: count(u16) + [name(str) + command(str) + created_at(u64) + alive(u8)]*
            let mut p = Vec::new();
            p.extend_from_slice(&(sessions.len() as u16).to_le_bytes());
            for s in sessions {
                write_str(&mut p, &s.name);
                write_str(&mut p, &s.command);
                p.extend_from_slice(&s.created_at.to_le_bytes());
                p.push(if s.alive { 1 } else { 0 });
            }
            Ok((MSG_SESSION_LIST, p))
        }
        Message::ScreenData { content, cursor_col, cursor_row } => {
            // UTF-8 content + cursor position (4 bytes)
            let mut p = Vec::with_capacity(content.len() + 4);
            p.extend_from_slice(content.as_bytes());
            p.extend_from_slice(&cursor_col.to_le_bytes());
            p.extend_from_slice(&cursor_row.to_le_bytes());
            Ok((MSG_SCREEN_DATA, p))
        }
        Message::Pong => Ok((MSG_PONG, Vec::new())),
    }
}

// ── Binary payload decoding ──────────────────────────────────────────

fn proto_err(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

fn decode(msg_type: i32, payload: &[u8]) -> std::io::Result<Message> {
    let mut cur = Cursor::new(payload);

    match msg_type {
        MSG_CREATE => {
            let cols = cur.read_u16()?;
            let rows = cur.read_u16()?;
            let name = cur.read_opt_str()?;
            let command = cur.read_opt_str()?;
            Ok(Message::Create { name, command, cols, rows })
        }
        MSG_ERROR => {
            let message = std::str::from_utf8(payload)
                .map_err(|_| proto_err("invalid UTF-8 in error message"))?
                .to_string();
            Ok(Message::Error { message })
        }
        MSG_WINCH => {
            let name = cur.read_str()?;
            let cols = cur.read_u16()?;
            let rows = cur.read_u16()?;
            Ok(Message::Winch { name, cols, rows })
        }
        MSG_HANGUP => {
            let name = std::str::from_utf8(payload)
                .map_err(|_| proto_err("invalid UTF-8 in session name"))?
                .to_string();
            Ok(Message::Hangup { name })
        }
        MSG_COMMAND => {
            let name = cur.read_str()?;
            let nargs = cur.read_u16()? as usize;
            let mut args = Vec::with_capacity(nargs);
            for _ in 0..nargs {
                args.push(cur.read_str()?);
            }
            Ok(Message::Command { name, args })
        }
        MSG_SEND_DATA => {
            let name = cur.read_str()?;
            let data = cur.remaining().to_vec();
            Ok(Message::SendData { name, data })
        }
        MSG_VIEW_SCREEN => {
            let name = std::str::from_utf8(payload)
                .map_err(|_| proto_err("invalid UTF-8 in session name"))?
                .to_string();
            Ok(Message::ViewScreen { name })
        }
        MSG_RESIZE_PTY => {
            let name = cur.read_str()?;
            let cols = cur.read_u16()?;
            let rows = cur.read_u16()?;
            Ok(Message::ResizePty { name, cols, rows })
        }
        MSG_KILL_SERVER => Ok(Message::KillServer),
        MSG_PING => Ok(Message::Ping),
        MSG_LIST_SESSIONS => Ok(Message::ListSessions),

        // Responses
        MSG_OK => Ok(Message::Ok),
        MSG_CREATED => {
            let name = std::str::from_utf8(payload)
                .map_err(|_| proto_err("invalid UTF-8 in session name"))?
                .to_string();
            Ok(Message::Created { name })
        }
        MSG_SESSION_LIST => {
            let count = cur.read_u16()? as usize;
            let mut sessions = Vec::with_capacity(count);
            for _ in 0..count {
                let name = cur.read_str()?;
                let command = cur.read_str()?;
                let created_at = cur.read_u64()?;
                let alive = cur.read_u8()? != 0;
                sessions.push(SessionInfo { name, command, created_at, alive });
            }
            Ok(Message::SessionList { sessions })
        }
        MSG_SCREEN_DATA => {
            if payload.len() < 4 {
                return Err(proto_err("screen data too short"));
            }
            let cursor_bytes = &payload[payload.len() - 4..];
            let cursor_col = u16::from_le_bytes([cursor_bytes[0], cursor_bytes[1]]);
            let cursor_row = u16::from_le_bytes([cursor_bytes[2], cursor_bytes[3]]);
            let content = std::str::from_utf8(&payload[..payload.len() - 4])
                .map_err(|_| proto_err("invalid UTF-8 in screen data"))?
                .to_string();
            Ok(Message::ScreenData { content, cursor_col, cursor_row })
        }
        MSG_PONG => Ok(Message::Pong),

        _ => Err(proto_err(&format!("unknown message type: {}", msg_type))),
    }
}

// ── Binary helpers ───────────────────────────────────────────────────

/// Write a length-prefixed string (u16-LE len + UTF-8 bytes).
fn write_str(buf: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    buf.extend_from_slice(&(b.len() as u16).to_le_bytes());
    buf.extend_from_slice(b);
}

/// Write an optional string: 0xFFFF = None, otherwise u16-LE len + bytes.
fn write_opt_str(buf: &mut Vec<u8>, s: Option<&str>) {
    match s {
        None => buf.extend_from_slice(&0xFFFFu16.to_le_bytes()),
        Some(s) => write_str(buf, s),
    }
}

/// Simple cursor for reading binary payloads.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    fn ensure(&self, n: usize) -> std::io::Result<()> {
        if self.pos + n > self.data.len() {
            Err(proto_err("unexpected end of payload"))
        } else {
            Ok(())
        }
    }

    fn read_u8(&mut self) -> std::io::Result<u8> {
        self.ensure(1)?;
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn read_u16(&mut self) -> std::io::Result<u16> {
        self.ensure(2)?;
        let v = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    fn read_u64(&mut self) -> std::io::Result<u64> {
        self.ensure(8)?;
        let v = u64::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
            self.data[self.pos + 4],
            self.data[self.pos + 5],
            self.data[self.pos + 6],
            self.data[self.pos + 7],
        ]);
        self.pos += 8;
        Ok(v)
    }

    fn read_str(&mut self) -> std::io::Result<String> {
        let len = self.read_u16()? as usize;
        self.ensure(len)?;
        let s = std::str::from_utf8(&self.data[self.pos..self.pos + len])
            .map_err(|_| proto_err("invalid UTF-8 in string field"))?;
        self.pos += len;
        Ok(s.to_string())
    }

    fn read_opt_str(&mut self) -> std::io::Result<Option<String>> {
        let raw = self.read_u16()?;
        if raw == 0xFFFF {
            return Ok(None);
        }
        let len = raw as usize;
        self.ensure(len)?;
        let s = std::str::from_utf8(&self.data[self.pos..self.pos + len])
            .map_err(|_| proto_err("invalid UTF-8 in string field"))?;
        self.pos += len;
        Ok(Some(s.to_string()))
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
