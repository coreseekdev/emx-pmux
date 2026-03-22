use clap::Parser;
use pmux::cli::{Args, Mode};
use pmux::ipc::{self, Request, Response};
use pmux::platform;
use std::process;

/// Parse C-style escape sequences in a string (\n, \r, \t, \\, \xHH).
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

/// Send a request to the daemon and return the response.
async fn rpc(req: &Request) -> Result<Response, String> {
    let stream =
        platform::connect_daemon().await.map_err(|e| format!("cannot connect to daemon: {}", e))?;
    let (mut rd, mut wr) = tokio::io::split(stream);
    ipc::send_msg(&mut wr, req).await.map_err(|e| format!("send failed: {}", e))?;
    ipc::recv_msg(&mut rd).await.map_err(|e| format!("recv failed: {}", e))
}

/// Create a new session (default mode).
async fn run_create(args: &Args) -> Result<(), String> {
    // Ensure daemon is running (auto-start).
    platform::ensure_daemon().await.map_err(|e| format!("failed to start daemon: {}", e))?;
    let resp = rpc(&Request::NewSession {
        name: args.session.clone(),
        command: args.command.clone(),
        cols: args.width,
        rows: args.height,
    }).await?;
    match resp {
        Response::Created { name } => {
            println!("{}", name);
            Ok(())
        }
        Response::Error { message } => Err(message),
        _ => Err("unexpected response".into()),
    }
}

/// List sessions.
async fn run_list() -> Result<(), String> {
    let resp = rpc(&Request::ListSessions).await?;
    match resp {
        Response::SessionList { sessions } => {
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
        Response::Error { message } => Err(message),
        _ => Err("unexpected response".into()),
    }
}

/// View/peek session screen buffer (non-interactive, unlike -r).
async fn run_view(args: &Args) -> Result<(), String> {
    let name = args.session
        .as_ref()
        .ok_or_else(|| "session name required (use -S)".to_string())?;
    let resp = rpc(&Request::ViewScreen { name: name.clone() }).await?;
    match resp {
        Response::Screen { content } => {
            println!("{}", content);
            Ok(())
        }
        Response::Error { message } => Err(message),
        _ => Err("unexpected response".into()),
    }
}

/// Resume/attach to a session (interactive, for future implementation).
async fn run_resume(args: &Args) -> Result<(), String> {
    let name = args.session
        .as_ref()
        .ok_or_else(|| "session name required (use -S)".to_string())?;
    let resp = rpc(&Request::ViewScreen { name: name.clone() }).await?;
    match resp {
        Response::Screen { content } => {
            println!("{}", content);
            Ok(())
        }
        Response::Error { message } => Err(message),
        _ => Err("unexpected response".into()),
    }
}

/// Execute command on running session (-X mode).
async fn run_exec(args: &Args) -> Result<(), String> {
    let name = args.session
        .as_ref()
        .ok_or_else(|| "session name required (use -S)".to_string())?;

    if args.command_args.is_empty() {
        return Err("command required (e.g., stuff, resize, view, hardcopy, quit)".into());
    }

    let cmd = args.command_args[0].clone();
    let cmd_args: Vec<&String> = args.command_args.iter().skip(1).collect();

    match cmd.as_str() {
        "stuff" => {
            if cmd_args.is_empty() {
                return Err("stuff requires text argument".into());
            }
            let text = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
            let data = unescape(&text);
            let resp = rpc(&Request::SendData {
                name: name.clone(),
                data,
            }).await?;
            match resp {
                Response::Ok => Ok(()),
                Response::Error { message } => Err(message),
                _ => Err("unexpected response".into()),
            }
        }
        "resize" => {
            if cmd_args.len() != 2 {
                return Err("resize requires width and height (e.g., resize 80 24)".into());
            }
            let width = cmd_args[0].parse::<u16>()
                .map_err(|_| "invalid width".to_string())?;
            let height = cmd_args[1].parse::<u16>()
                .map_err(|_| "invalid height".to_string())?;
            let resp = rpc(&Request::ResizePty {
                name: name.clone(),
                cols: width,
                rows: height,
            }).await?;
            match resp {
                Response::Ok => Ok(()),
                Response::Error { message } => Err(message),
                _ => Err("unexpected response".into()),
            }
        }
        "view" => {
            let resp = rpc(&Request::ViewScreen { name: name.clone() }).await?;
            match resp {
                Response::Screen { content } => {
                    println!("{}", content);
                    Ok(())
                }
                Response::Error { message } => Err(message),
                _ => Err("unexpected response".into()),
            }
        }
        "hardcopy" => {
            // hardcopy [file] - if file is "-", output to stdout
            let file = if !cmd_args.is_empty() {
                cmd_args[0].clone()
            } else {
                "-".to_string()  // default to stdout
            };
            let resp = rpc(&Request::ViewScreen { name: name.clone() }).await?;
            match resp {
                Response::Screen { content } => {
                    if file == "-" {
                        println!("{}", content);
                    } else {
                        std::fs::write(&file, content)
                            .map_err(|e| format!("failed to write hardcopy: {}", e))?;
                        eprintln!("hardcopy written to {}", file);
                    }
                    Ok(())
                }
                Response::Error { message } => Err(message),
                _ => Err("unexpected response".into()),
            }
        }
        "quit" => {
            let resp = rpc(&Request::KillSession { name: name.clone() }).await?;
            match resp {
                Response::Ok => Ok(()),
                Response::Error { message } => Err(message),
                _ => Err("unexpected response".into()),
            }
        }
        _ => Err(format!("unknown command: {}", cmd)),
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
    let resp = rpc(&Request::KillServer).await?;
    match resp {
        Response::Ok => {
            println!("daemon stopped");
            Ok(())
        }
        Response::Error { message } => Err(message),
        _ => Err("unexpected response".into()),
    }
}

/// Ping the daemon.
async fn run_ping() -> Result<(), String> {
    let resp = rpc(&Request::Ping).await?;
    match resp {
        Response::Pong => {
            println!("pong");
            Ok(())
        }
        _ => Err("unexpected response".into()),
    }
}
