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

const MAX_MSG: usize = 16 * 1024 * 1024; // 16 MiB

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

    /// MSG_SCREEN_DATA: Screen buffer content as raw UTF-8 bytes.
    ScreenData { content: String },

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
        Message::ScreenData { content } => {
            // Raw UTF-8
            Ok((MSG_SCREEN_DATA, content.as_bytes().to_vec()))
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
            let content = std::str::from_utf8(payload)
                .map_err(|_| proto_err("invalid UTF-8 in screen data"))?
                .to_string();
            Ok(Message::ScreenData { content })
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
mod tests {
    use super::*;

    /// Round-trip test: encode → decode for every message variant.
    #[test]
    fn roundtrip_all_variants() {
        let messages: Vec<Message> = vec![
            Message::Create {
                name: Some("test".into()),
                command: Some("bash -c 'echo hello'".into()),
                cols: 120,
                rows: 40,
            },
            Message::Create {
                name: None,
                command: None,
                cols: 80,
                rows: 24,
            },
            Message::Error { message: "session not found".into() },
            Message::Winch { name: "0".into(), cols: 200, rows: 50 },
            Message::Hangup { name: "mysession".into() },
            Message::Command {
                name: "0".into(),
                args: vec!["stuff".into(), "hello world\\n".into()],
            },
            Message::SendData {
                name: "0".into(),
                data: vec![0x1b, 0x5b, 0x41], // ESC [ A
            },
            Message::ViewScreen { name: "0".into() },
            Message::ResizePty { name: "0".into(), cols: 132, rows: 43 },
            Message::KillServer,
            Message::Ping,
            Message::ListSessions,
            Message::Ok,
            Message::Created { name: "0".into() },
            Message::SessionList {
                sessions: vec![
                    SessionInfo {
                        name: "0".into(),
                        command: "bash".into(),
                        created_at: 1700000000,
                        alive: true,
                    },
                    SessionInfo {
                        name: "build".into(),
                        command: "cargo build".into(),
                        created_at: 1700000100,
                        alive: false,
                    },
                ],
            },
            Message::ScreenData { content: "$ hello\nworld".into() },
            Message::Pong,
        ];

        for msg in &messages {
            let (msg_type, payload) = encode(msg).expect("encode failed");
            let decoded = decode(msg_type, &payload).expect("decode failed");
            assert_msg_eq(msg, &decoded);
        }
    }

    /// Verify the protocol revision magic value.
    #[test]
    fn protocol_revision_magic() {
        // 'p'=0x70, 'm'=0x6d, 'x'=0x78, version=1
        assert_eq!(PROTOCOL_REVISION, 0x706d_7801_u32 as i32);
    }

    /// Verify Screen-compatible MSG_* constants.
    #[test]
    fn screen_compatible_constants() {
        assert_eq!(MSG_CREATE, 0);
        assert_eq!(MSG_ERROR, 1);
        assert_eq!(MSG_ATTACH, 2);
        assert_eq!(MSG_DETACH, 4);
        assert_eq!(MSG_WINCH, 6);
        assert_eq!(MSG_HANGUP, 7);
        assert_eq!(MSG_COMMAND, 8);
        assert_eq!(MSG_QUERY, 9);
    }

    /// Async round-trip through write_msg / read_msg.
    #[tokio::test]
    async fn async_roundtrip() {
        let msg = Message::SendData {
            name: "test".into(),
            data: b"hello\x00world".to_vec(),
        };

        let mut buf = Vec::new();
        write_msg(&mut buf, &msg).await.unwrap();

        let mut cursor = &buf[..];
        let decoded = read_msg(&mut cursor).await.unwrap();

        match decoded {
            Message::SendData { name, data } => {
                assert_eq!(name, "test");
                assert_eq!(data, b"hello\x00world");
            }
            _ => panic!("expected SendData"),
        }
    }

    /// Verify bad magic is rejected.
    #[tokio::test]
    async fn reject_bad_revision() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0x12345678i32.to_le_bytes()); // bad magic
        buf.extend_from_slice(&0i32.to_le_bytes());          // type
        buf.extend_from_slice(&0u32.to_le_bytes());          // len

        let mut cursor = &buf[..];
        let result = read_msg(&mut cursor).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid message"));
    }

    fn assert_msg_eq(a: &Message, b: &Message) {
        // Compare discriminant and key fields
        match (a, b) {
            (Message::Create { name: n1, command: c1, cols: co1, rows: r1 },
             Message::Create { name: n2, command: c2, cols: co2, rows: r2 }) => {
                assert_eq!(n1, n2);
                assert_eq!(c1, c2);
                assert_eq!(co1, co2);
                assert_eq!(r1, r2);
            }
            (Message::Error { message: m1 }, Message::Error { message: m2 }) => {
                assert_eq!(m1, m2);
            }
            (Message::Winch { name: n1, cols: c1, rows: r1 },
             Message::Winch { name: n2, cols: c2, rows: r2 }) => {
                assert_eq!(n1, n2);
                assert_eq!(c1, c2);
                assert_eq!(r1, r2);
            }
            (Message::Hangup { name: n1 }, Message::Hangup { name: n2 }) => {
                assert_eq!(n1, n2);
            }
            (Message::Command { name: n1, args: a1 },
             Message::Command { name: n2, args: a2 }) => {
                assert_eq!(n1, n2);
                assert_eq!(a1, a2);
            }
            (Message::SendData { name: n1, data: d1 },
             Message::SendData { name: n2, data: d2 }) => {
                assert_eq!(n1, n2);
                assert_eq!(d1, d2);
            }
            (Message::ViewScreen { name: n1 }, Message::ViewScreen { name: n2 }) => {
                assert_eq!(n1, n2);
            }
            (Message::ResizePty { name: n1, cols: c1, rows: r1 },
             Message::ResizePty { name: n2, cols: c2, rows: r2 }) => {
                assert_eq!(n1, n2);
                assert_eq!(c1, c2);
                assert_eq!(r1, r2);
            }
            (Message::KillServer, Message::KillServer) => {}
            (Message::Ping, Message::Ping) => {}
            (Message::ListSessions, Message::ListSessions) => {}
            (Message::Ok, Message::Ok) => {}
            (Message::Created { name: n1 }, Message::Created { name: n2 }) => {
                assert_eq!(n1, n2);
            }
            (Message::SessionList { sessions: s1 },
             Message::SessionList { sessions: s2 }) => {
                assert_eq!(s1.len(), s2.len());
                for (a, b) in s1.iter().zip(s2.iter()) {
                    assert_eq!(a.name, b.name);
                    assert_eq!(a.command, b.command);
                    assert_eq!(a.created_at, b.created_at);
                    assert_eq!(a.alive, b.alive);
                }
            }
            (Message::ScreenData { content: c1 }, Message::ScreenData { content: c2 }) => {
                assert_eq!(c1, c2);
            }
            (Message::Pong, Message::Pong) => {}
            _ => panic!("message variant mismatch: {:?} vs {:?}", a, b),
        }
    }
}
