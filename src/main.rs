use clap::Parser;
use pmux::cli::{Args, Mode};
use pmux::ipc::{self, Message};
use pmux::platform;
use pmux::pmux_log;
use pmux::terminal;
use std::process;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
    pmux_log!("rpc: sending {:?}", msg);
    let stream =
        platform::connect_daemon().await.map_err(|e| format!("cannot connect to daemon: {}", e))?;
    let (mut rd, mut wr) = tokio::io::split(stream);
    ipc::write_msg(&mut wr, msg).await.map_err(|e| format!("send failed: {}", e))?;
    let resp = ipc::read_msg(&mut rd).await.map_err(|e| format!("recv failed: {}", e))?;
    pmux_log!("rpc: received {:?}", resp);
    Ok(resp)
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

/// Resume/attach to a session (interactive terminal).
///
/// Enters terminal raw mode and runs a bidirectional loop:
/// - stdin keystrokes are forwarded to the PTY via MSG_SEND_DATA
/// - screen content is polled and rendered to stdout
/// - Ctrl-A then 'd' detaches (Screen-compatible)
async fn run_resume(args: &Args) -> Result<(), String> {
    let name = args.session
        .as_ref()
        .ok_or_else(|| "session name required (use -S)".to_string())?;

    // Ensure daemon is running.
    platform::ensure_daemon().await
        .map_err(|e| format!("failed to start daemon: {}", e))?;

    // Verify session exists by fetching initial screen content.
    let resp = rpc(&Message::ViewScreen { name: name.clone() }).await?;
    let initial = match resp {
        Message::ScreenData { content } => {
            pmux_log!("resume: initial screen len={}", content.len());
            content
        }
        Message::Error { message } => return Err(message),
        _ => return Err("unexpected response".into()),
    };

    // Enter raw mode.
    let saved = terminal::enable_raw_mode()
        .map_err(|e| format!("failed to enable raw mode: {}", e))?;

    let result = attach_loop(name, &initial).await;

    // Always restore terminal state.
    let _ = terminal::disable_raw_mode(&saved);

    // Print detach/exit message on a clean line.
    match &result {
        Ok(()) => eprintln!("\r\n[detached from session {}]", name),
        Err(e) if e.contains("exited") || e.contains("not found") || e.contains("lost connection") => {
            eprintln!("\r\n[session {} ended]", name);
            return Ok(()); // PTY exited — not an error
        }
        Err(e) => eprintln!("\r\n[session {}: {}]", name, e),
    }
    result
}

/// Main attach loop: forward stdin → PTY, poll screen → stdout.
async fn attach_loop(name: &str, initial: &str) -> Result<(), String> {
    let mut stdout = tokio::io::stdout();

    // Clear screen and render initial content.
    render_screen(&mut stdout, initial).await?;

    let mut last_content = initial.to_string();
    let mut stdin = tokio::io::stdin();
    let mut buf = [0u8; 1024];
    let mut ctrl_a_pending = false;

    let mut refresh = tokio::time::interval(std::time::Duration::from_millis(33));
    // Don't let missed ticks pile up.
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased; // Prioritize user input over refresh

            result = stdin.read(&mut buf) => {
                let n = result.map_err(|e| format!("stdin read: {}", e))?;
                if n == 0 {
                    return Ok(()); // EOF on stdin
                }

                let mut to_send = Vec::with_capacity(n);
                for &b in &buf[..n] {
                    if ctrl_a_pending {
                        ctrl_a_pending = false;
                        match b {
                            b'd' => return Ok(()), // Ctrl-A d = detach
                            0x01 => to_send.push(0x01), // Ctrl-A Ctrl-A = literal Ctrl-A
                            other => {
                                // Not a recognized escape — send the Ctrl-A and this byte
                                to_send.push(0x01);
                                to_send.push(other);
                            }
                        }
                    } else if b == 0x01 {
                        ctrl_a_pending = true;
                    } else {
                        to_send.push(b);
                    }
                }

                if !to_send.is_empty() {
                    match rpc(&Message::SendData {
                        name: name.to_string(),
                        data: to_send,
                    }).await {
                        Ok(Message::Ok) => {}
                        Ok(Message::Error { message }) => return Err(message),
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }
            }

            _ = refresh.tick() => {
                match rpc(&Message::ViewScreen { name: name.to_string() }).await {
                    Ok(Message::ScreenData { content }) => {
                        if content != last_content {
                            pmux_log!("attach: screen changed, len={}", content.len());
                            render_screen(&mut stdout, &content).await?;
                            last_content = content;
                        }
                    }
                    Ok(Message::Error { message }) => {
                        // Session likely died
                        return Err(message);
                    }
                    Err(_) => {
                        // Daemon connection lost
                        return Err("lost connection to daemon".into());
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Render screen content to stdout using ANSI positioning.
async fn render_screen(stdout: &mut tokio::io::Stdout, content: &str) -> Result<(), String> {
    let mut buf = Vec::with_capacity(content.len() + 256);
    buf.extend_from_slice(b"\x1b[H"); // cursor home
    for (i, line) in content.split('\n').enumerate() {
        if i > 0 {
            buf.extend_from_slice(b"\r\n");
        }
        buf.extend_from_slice(line.as_bytes());
        buf.extend_from_slice(b"\x1b[K"); // clear to end of line
    }
    buf.extend_from_slice(b"\x1b[J"); // clear below
    stdout.write_all(&buf).await.map_err(|e| format!("write: {}", e))?;
    stdout.flush().await.map_err(|e| format!("flush: {}", e))?;
    Ok(())
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
