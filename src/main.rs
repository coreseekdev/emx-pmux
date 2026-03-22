use clap::Parser;
use pmux::cli::{Args, Mode};
use pmux::ipc::{self, Request, Response};
use pmux::platform;
use std::process;

fn main() {
    let args = Args::parse();
    let mode = args.mode();

    let result = match mode {
        Mode::Daemon => pmux::daemon::run().map_err(|e| e.to_string()),
        Mode::Create => run_create(&args),
        Mode::List => run_list(),
        Mode::View => run_view(&args),
        Mode::Resume => run_resume(&args),
        Mode::Exec => run_exec(&args),
        Mode::Status => run_status(),
        Mode::Stop => run_stop(),
        Mode::Ping => run_ping(),
    };

    if let Err(e) = result {
        eprintln!("error: {}", e);
        process::exit(1);
    }
}

/// Send a request to the daemon and return the response.
fn rpc(req: &Request) -> Result<Response, String> {
    let mut stream =
        platform::connect_daemon().map_err(|e| format!("cannot connect to daemon: {}", e))?;
    ipc::send_msg(&mut stream, req).map_err(|e| format!("send failed: {}", e))?;
    ipc::recv_msg(&mut stream).map_err(|e| format!("recv failed: {}", e))
}

/// Create a new session (default mode).
fn run_create(args: &Args) -> Result<(), String> {
    // Ensure daemon is running (auto-start).
    platform::ensure_daemon().map_err(|e| format!("failed to start daemon: {}", e))?;
    let resp = rpc(&Request::NewSession {
        name: args.session.clone(),
        command: args.command.clone(),
        cols: args.width,
        rows: args.height,
    })?;
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
fn run_list() -> Result<(), String> {
    let resp = rpc(&Request::ListSessions)?;
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
fn run_view(args: &Args) -> Result<(), String> {
    let name = args.session
        .as_ref()
        .ok_or_else(|| "session name required (use -S)".to_string())?;
    let resp = rpc(&Request::ViewScreen { name: name.clone() })?;
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
fn run_resume(args: &Args) -> Result<(), String> {
    let name = args.session
        .as_ref()
        .ok_or_else(|| "session name required (use -S)".to_string())?;
    let resp = rpc(&Request::ViewScreen { name: name.clone() })?;
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
fn run_exec(args: &Args) -> Result<(), String> {
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
            let resp = rpc(&Request::SendData {
                name: name.clone(),
                data: text.into_bytes(),
            })?;
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
            })?;
            match resp {
                Response::Ok => Ok(()),
                Response::Error { message } => Err(message),
                _ => Err("unexpected response".into()),
            }
        }
        "view" => {
            let resp = rpc(&Request::ViewScreen { name: name.clone() })?;
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
            let resp = rpc(&Request::ViewScreen { name: name.clone() })?;
            match resp {
                Response::Screen { content } => {
                    if file == "-" {
                        // Output to stdout
                        println!("{}", content);
                    } else {
                        // Write to file
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
            let resp = rpc(&Request::KillSession { name: name.clone() })?;
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
        println!("daemon is not running");
        process::exit(1);
    }
}

/// Stop the daemon.
fn run_stop() -> Result<(), String> {
    let resp = rpc(&Request::KillServer)?;
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
fn run_ping() -> Result<(), String> {
    let resp = rpc(&Request::Ping)?;
    match resp {
        Response::Pong => {
            println!("pong");
            Ok(())
        }
        _ => Err("unexpected response".into()),
    }
}
