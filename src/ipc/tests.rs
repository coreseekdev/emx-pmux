use super::*;
use crate::consts::{DEFAULT_COLS, DEFAULT_ROWS};
use crate::session::SessionInfo;

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
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
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
        Message::ScreenData { content: "$ hello\nworld".into(), cursor_col: 7, cursor_row: 1 },
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
        (Message::ScreenData { content: c1, cursor_col: cc1, cursor_row: cr1 },
         Message::ScreenData { content: c2, cursor_col: cc2, cursor_row: cr2 }) => {
            assert_eq!(c1, c2);
            assert_eq!(cc1, cc2);
            assert_eq!(cr1, cr2);
        }
        (Message::Pong, Message::Pong) => {}
        _ => panic!("message variant mismatch: {:?} vs {:?}", a, b),
    }
}
