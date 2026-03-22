use clap::Parser;
use pmux::cli::{Args, Mode};
use pmux::ipc::{self, Message};
use pmux::platform;
use std::process;

/// Parse C-style escape sequences in a string (\n, \r, \t, \\, \e, \xHH, \uXXXX).
fn unescape(s: &str) -> Vec<u8> {
    let mut result = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push(b'\n'),
                Some('r') => result.push(b'\r'),
                Some('t') => result.push(b'\t'),
                Some('\\') => result.push(b'\\'),
                Some('0') => result.push(0),
                Some('a') => result.push(0x07),
                Some('e') | Some('E') => result.push(0x1B),
                Some('u') => {
                    let mut hex = String::new();
                    for _ in 0..4 {
                        if let Some(&next) = chars.as_str().as_bytes().first() {
                            if (next as char).is_ascii_hexdigit() {
                                hex.push(chars.next().unwrap());
                            } else {
                                break;
                            }
                        }
                    }
                    if let Ok(cp) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(cp) {
                            let mut buf = [0u8; 4];
                            result.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                    } else {
                        result.push(b'\\');
                        result.push(b'u');
                        result.extend(hex.as_bytes());
                    }
                }
                Some('x') => {
                    let mut hex = String::new();
                    for _ in 0..2 {
                        if let Some(&next) = chars.as_str().as_bytes().first() {
                            if (next as char).is_ascii_hexdigit() {
                                hex.push(chars.next().unwrap());
                            } else {
                                break;
                            }
                        }
                    }
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        result.push(byte);
                    } else {
                        result.push(b'\\');
                        result.push(b'x');
                        result.extend(hex.as_bytes());
                    }
                }
                Some(other) => {
                    result.push(b'\\');
                    let mut buf = [0u8; 4];
                    result.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                }
                None => result.push(b'\\'),
            }
        } else {
            let mut buf = [0u8; 4];
            result.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    result
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let mode = args.mode();

    let result = match mode {
        Mode::Daemon => pmux::daemon::run().await.map_err(|e| e.to_string()),
        Mode::Create => run_create(&args).await,
        Mode::List => run_list().await,
        Mode::View => run_view(&args).await,
        Mode::Resume => run_resume(&args).await,
        Mode::Exec => run_exec(&args).await,
        Mode::Status => run_status(),
        Mode::Stop => run_stop().await,
        Mode::Ping => run_ping().await,
    };

    if let Err(e) = result {
        eprintln!("error: {}", e);
        process::exit(1);
    }
}

/// Send a message to the daemon and return the response.
async fn rpc(msg: &Message) -> Result<Message, String> {
    let stream =
        platform::connect_daemon().await.map_err(|e| format!("cannot connect to daemon: {}", e))?;
    let (mut rd, mut wr) = tokio::io::split(stream);
    ipc::write_msg(&mut wr, msg).await.map_err(|e| format!("send failed: {}", e))?;
    ipc::read_msg(&mut rd).await.map_err(|e| format!("recv failed: {}", e))
}

/// Create a new session (default mode).
async fn run_create(args: &Args) -> Result<(), String> {
    // Ensure daemon is running (auto-start).
    platform::ensure_daemon().await.map_err(|e| format!("failed to start daemon: {}", e))?;
    let resp = rpc(&Message::Create {
        name: args.session.clone(),
        command: args.command.clone(),
        cols: args.width,
        rows: args.height,
    }).await?;
    match resp {
        Message::Created { name } => {
            println!("{}", name);
            Ok(())
        }
        Message::Error { message } => Err(message),
        _ => Err("unexpected response".into()),
    }
}

/// List sessions.
async fn run_list() -> Result<(), String> {
    let resp = rpc(&Message::ListSessions).await?;
    match resp {
        Message::SessionList { sessions } => {
            if sessions.is_empty() {
                println!("No sessions");
            } else {
                for s in sessions {
                    let status = if s.alive { "running" } else { "dead" };
                    println!("{}: {} [{}]", s.name, s.command, status);
                }
            }
            Ok(())
        }
        Message::Error { message } => Err(message),
        _ => Err("unexpected response".into()),
    }
}

/// View/peek session screen buffer (non-interactive, unlike -r).
async fn run_view(args: &Args) -> Result<(), String> {
    let name = args.session
        .as_ref()
        .ok_or_else(|| "session name required (use -S)".to_string())?;
    let resp = rpc(&Message::ViewScreen { name: name.clone() }).await?;
    match resp {
        Message::ScreenData { content } => {
            println!("{}", content);
            Ok(())
        }
        Message::Error { message } => Err(message),
        _ => Err("unexpected response".into()),
    }
}

/// Resume/attach to a session (interactive, for future implementation).
async fn run_resume(args: &Args) -> Result<(), String> {
    let name = args.session
        .as_ref()
        .ok_or_else(|| "session name required (use -S)".to_string())?;
    let resp = rpc(&Message::ViewScreen { name: name.clone() }).await?;
    match resp {
        Message::ScreenData { content } => {
            println!("{}", content);
            Ok(())
        }
        Message::Error { message } => Err(message),
        _ => Err("unexpected response".into()),
    }
}

/// Execute command on running session (-X mode).
///
/// Uses MSG_COMMAND (Screen-compatible) for most commands.
/// `stuff` with escape sequences uses MSG_SEND_DATA for raw binary.
async fn run_exec(args: &Args) -> Result<(), String> {
    let name = args.session
        .as_ref()
        .ok_or_else(|| "session name required (use -S)".to_string())?;

    if args.command_args.is_empty() {
        return Err("command required (e.g., stuff, resize, view, hardcopy, quit)".into());
    }

    let cmd = &args.command_args[0];

    // Special case: `stuff` with escape processing uses MSG_SEND_DATA
    // so that raw binary (including NUL, ESC) reaches the PTY correctly.
    if cmd == "stuff" {
        let cmd_args: Vec<&str> = args.command_args[1..].iter().map(|s| s.as_str()).collect();
        if cmd_args.is_empty() {
            return Err("stuff requires text argument".into());
        }
        let text = cmd_args.join(" ");
        let data = unescape(&text);
        let resp = rpc(&Message::SendData {
            name: name.clone(),
            data,
        }).await?;
        return match resp {
            Message::Ok => Ok(()),
            Message::Error { message } => Err(message),
            _ => Err("unexpected response".into()),
        };
    }

    // Special case: `hardcopy` with file argument needs client-side file write
    if cmd == "hardcopy" {
        let file = if args.command_args.len() > 1 {
            args.command_args[1].clone()
        } else {
            "-".to_string()
        };
        // Use MSG_COMMAND to get screen content
        let resp = rpc(&Message::Command {
            name: name.clone(),
            args: vec!["view".into()],
        }).await?;
        return match resp {
            Message::ScreenData { content } => {
                if file == "-" {
                    println!("{}", content);
                } else {
                    std::fs::write(&file, &content)
                        .map_err(|e| format!("failed to write hardcopy: {}", e))?;
                    eprintln!("hardcopy written to {}", file);
                }
                Ok(())
            }
            Message::Error { message } => Err(message),
            _ => Err("unexpected response".into()),
        };
    }

    // All other commands: send as MSG_COMMAND (Screen-compatible)
    let resp = rpc(&Message::Command {
        name: name.clone(),
        args: args.command_args.clone(),
    }).await?;

    match resp {
        Message::Ok => Ok(()),
        Message::ScreenData { content } => {
            println!("{}", content);
            Ok(())
        }
        Message::Error { message } => Err(message),
        _ => Err("unexpected response".into()),
    }
}

/// Show daemon status.
fn run_status() -> Result<(), String> {
    if platform::is_daemon_running() {
        println!("daemon is running");
        Ok(())
    } else {
        eprintln!("daemon is not running");
        Err("daemon is not running".into())
    }
}

/// Stop the daemon.
async fn run_stop() -> Result<(), String> {
    let resp = rpc(&Message::KillServer).await?;
    match resp {
        Message::Ok => {
            println!("daemon stopped");
            Ok(())
        }
        Message::Error { message } => Err(message),
        _ => Err("unexpected response".into()),
    }
}

/// Ping the daemon.
async fn run_ping() -> Result<(), String> {
    let resp = rpc(&Message::Ping).await?;
    match resp {
        Message::Pong => {
            println!("pong");
            Ok(())
        }
        _ => Err("unexpected response".into()),
    }
}
