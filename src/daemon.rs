//! Daemon: async event loop that manages sessions via IPC.
//!
//! Uses `tokio::select!` for multiplexing accept, shutdown signals,
//! and periodic dead-session reaping. Auto-exits when all sessions
//! have ended (after at least one was created).

use std::io;

use crate::ipc::{self, Message};
use crate::platform;
use crate::session::SessionManager;

/// Run the daemon main loop (async). Does not return until shutdown.
pub async fn run() -> io::Result<()> {
    platform::write_pid_file()?;
    let mut mgr = SessionManager::new();
    let mut had_sessions = false;
    let mut reap_interval = tokio::time::interval(std::time::Duration::from_secs(2));

    #[cfg(unix)]
    {
        run_unix(&mut mgr, &mut had_sessions, &mut reap_interval).await?;
    }
    #[cfg(windows)]
    {
        run_windows(&mut mgr, &mut had_sessions, &mut reap_interval).await?;
    }

    platform::remove_pid_file();
    Ok(())
}

// ── Unix (UnixListener) ─────────────────────────────────────────────

#[cfg(unix)]
async fn run_unix(
    mgr: &mut SessionManager,
    had_sessions: &mut bool,
    reap_interval: &mut tokio::time::Interval,
) -> io::Result<()> {
    let listener = platform::create_listener().await?;

    // Ignore SIGINT in daemon – exit via KillServer, SIGTERM, or auto-exit.
    unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN); }

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let (mut rd, mut wr) = tokio::io::split(stream);
                        if let Some(true) = handle_one(&mut rd, &mut wr, mgr).await {
                            break;
                        }
                    }
                    Err(_) => {} // transient accept error
                }
            }
            _ = sigterm.recv() => {
                break;
            }
            _ = reap_interval.tick() => {
                mgr.reap_dead();
                if *had_sessions && mgr.count() == 0 {
                    break;
                }
                if mgr.count() > 0 {
                    *had_sessions = true;
                }
            }
        }
    }
    Ok(())
}

// ── Windows (NamedPipe) ──────────────────────────────────────────────

#[cfg(windows)]
async fn run_windows(
    mgr: &mut SessionManager,
    had_sessions: &mut bool,
    reap_interval: &mut tokio::time::Interval,
) -> io::Result<()> {
    let mut pipe = platform::create_pipe_instance()?;
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            result = pipe.connect() => {
                if result.is_ok() {
                    let (mut rd, mut wr) = tokio::io::split(pipe);
                    let kill = handle_one(&mut rd, &mut wr, mgr).await;
                    // Create a fresh pipe instance for the next client.
                    pipe = platform::create_next_pipe_instance()?;
                    if kill == Some(true) {
                        break;
                    }
                } else {
                    pipe = platform::create_next_pipe_instance()?;
                }
            }
            _ = &mut ctrl_c => {
                break;
            }
            _ = reap_interval.tick() => {
                mgr.reap_dead();
                if *had_sessions && mgr.count() == 0 {
                    break;
                }
                if mgr.count() > 0 {
                    *had_sessions = true;
                }
            }
        }
    }
    Ok(())
}

// ── Request handling ─────────────────────────────────────────────────

/// Handle a single client message. Returns `Some(true)` on KillServer.
async fn handle_one<R, W>(rd: &mut R, wr: &mut W, mgr: &mut SessionManager) -> Option<bool>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let msg: Message = match ipc::read_msg(rd).await {
        Ok(m) => m,
        Err(_) => return None,
    };

    let (resp, kill) = dispatch(msg, mgr);
    let _ = ipc::write_msg(wr, &resp).await;
    Some(kill)
}

/// Dispatch a message, returning (response, should_kill_server).
fn dispatch(msg: Message, mgr: &mut SessionManager) -> (Message, bool) {
    match msg {
        Message::Create {
            name,
            command,
            cols,
            rows,
        } => match mgr.create(name, command, cols, rows) {
            Ok(n) => (Message::Created { name: n }, false),
            Err(e) => (Message::Error { message: e }, false),
        },

        Message::Hangup { name } => match mgr.kill(&name) {
            Ok(()) => (Message::Ok, false),
            Err(e) => (Message::Error { message: e }, false),
        },

        Message::ListSessions => {
            let sessions = mgr.list();
            (Message::SessionList { sessions }, false)
        }

        Message::SendData { name, data } => match mgr.send(&name, &data) {
            Ok(()) => (Message::Ok, false),
            Err(e) => (Message::Error { message: e }, false),
        },

        Message::ViewScreen { name } => match mgr.view(&name) {
            Ok(content) => (Message::ScreenData { content }, false),
            Err(e) => (Message::Error { message: e }, false),
        },

        Message::ResizePty { name, cols, rows }
        | Message::Winch { name, cols, rows } => match mgr.resize(&name, cols, rows) {
            Ok(()) => (Message::Ok, false),
            Err(e) => (Message::Error { message: e }, false),
        },

        Message::Command { name, args } => dispatch_command(&name, &args, mgr),

        Message::KillServer => (Message::Ok, true),

        Message::Ping => (Message::Pong, false),

        // Response messages should not arrive from clients — ignore them.
        _ => (Message::Error { message: "unexpected message".into() }, false),
    }
}

/// Dispatch a Screen-style command (MSG_COMMAND).
///
/// Screen commands: stuff, resize, hardcopy, quit, etc.
/// This allows `pmux -S name -X stuff "text"` to map to MSG_COMMAND.
fn dispatch_command(name: &str, args: &[String], mgr: &mut SessionManager) -> (Message, bool) {
    if args.is_empty() {
        return (Message::Error { message: "empty command".into() }, false);
    }
    let cmd = &args[0];
    let cmd_args = &args[1..];

    match cmd.as_str() {
        "stuff" => {
            let text = cmd_args.join(" ");
            match mgr.send(name, text.as_bytes()) {
                Ok(()) => (Message::Ok, false),
                Err(e) => (Message::Error { message: e }, false),
            }
        }
        "resize" => {
            if cmd_args.len() != 2 {
                return (Message::Error { message: "resize requires width height".into() }, false);
            }
            let cols = match cmd_args[0].parse::<u16>() {
                Ok(v) => v,
                Err(_) => return (Message::Error { message: "invalid width".into() }, false),
            };
            let rows = match cmd_args[1].parse::<u16>() {
                Ok(v) => v,
                Err(_) => return (Message::Error { message: "invalid height".into() }, false),
            };
            match mgr.resize(name, cols, rows) {
                Ok(()) => (Message::Ok, false),
                Err(e) => (Message::Error { message: e }, false),
            }
        }
        "view" | "hardcopy" => match mgr.view(name) {
            Ok(content) => (Message::ScreenData { content }, false),
            Err(e) => (Message::Error { message: e }, false),
        },
        "quit" | "kill" => match mgr.kill(name) {
            Ok(()) => (Message::Ok, false),
            Err(e) => (Message::Error { message: e }, false),
        },
        _ => (Message::Error { message: format!("unknown command: {}", cmd) }, false),
    }
}
