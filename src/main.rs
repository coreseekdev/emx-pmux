use clap::Parser;
use pmux::cli::{Args, Cmd};
use pmux::ipc::{self, Request, Response};
use pmux::platform;
use std::process;

fn main() {
    let args = Args::parse();

    let result = match args.command {
        Cmd::Daemon => {
            // Internal: run as the daemon process.
            pmux::daemon::run().map_err(|e| e.to_string())
        }
        cmd => run_client(cmd),
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

/// Run a client-side subcommand.
fn run_client(cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::New {
            target,
            command,
            width,
            height,
        } => {
            // Ensure daemon is running (auto-start).
            platform::ensure_daemon().map_err(|e| format!("failed to start daemon: {}", e))?;
            let resp = rpc(&Request::NewSession {
                name: target,
                command,
                cols: width,
                rows: height,
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

        Cmd::Kill { target } => {
            let resp = rpc(&Request::KillSession { name: target })?;
            match resp {
                Response::Ok => Ok(()),
                Response::Error { message } => Err(message),
                _ => Err("unexpected response".into()),
            }
        }

        Cmd::Ls => {
            let resp = rpc(&Request::ListSessions)?;
            match resp {
                Response::SessionList { sessions } => {
                    if sessions.is_empty() {
                        println!("no sessions");
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

        Cmd::Send { target, text } => {
            let data = text.join(" ").into_bytes();
            let resp = rpc(&Request::SendData {
                name: target,
                data,
            })?;
            match resp {
                Response::Ok => Ok(()),
                Response::Error { message } => Err(message),
                _ => Err("unexpected response".into()),
            }
        }

        Cmd::View { target } => {
            let resp = rpc(&Request::ViewScreen { name: target })?;
            match resp {
                Response::Screen { content } => {
                    println!("{}", content);
                    Ok(())
                }
                Response::Error { message } => Err(message),
                _ => Err("unexpected response".into()),
            }
        }

        Cmd::Resize {
            target,
            width,
            height,
        } => {
            let resp = rpc(&Request::ResizePty {
                name: target,
                cols: width,
                rows: height,
            })?;
            match resp {
                Response::Ok => Ok(()),
                Response::Error { message } => Err(message),
                _ => Err("unexpected response".into()),
            }
        }

        Cmd::Status => {
            if platform::is_daemon_running() {
                println!("daemon is running");
            } else {
                println!("daemon is not running");
                process::exit(1);
            }
            Ok(())
        }

        Cmd::Stop => {
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

        Cmd::Ping => {
            let resp = rpc(&Request::Ping)?;
            match resp {
                Response::Pong => {
                    println!("pong");
                    Ok(())
                }
                _ => Err("unexpected response".into()),
            }
        }

        Cmd::Daemon => unreachable!(),
    }
}
