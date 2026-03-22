//! Daemon: async event loop that manages sessions via IPC.
//!
//! Uses `tokio::select!` for multiplexing accept, shutdown signals,
//! and periodic dead-session reaping. Auto-exits when all sessions
//! have ended (after at least one was created).

use std::io;

use crate::ipc::{self, Request, Response};
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

    // Ignore SIGINT in daemon – exit via KillServer or auto-exit.
    unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN); }

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

/// Handle a single client request. Returns `Some(true)` on KillServer.
async fn handle_one<R, W>(rd: &mut R, wr: &mut W, mgr: &mut SessionManager) -> Option<bool>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let req: Request = match ipc::recv_msg(rd).await {
        Ok(r) => r,
        Err(_) => return None,
    };

    let (resp, kill) = dispatch(req, mgr);
    let _ = ipc::send_msg(wr, &resp).await;
    Some(kill)
}

/// Dispatch a request, returning (response, should_kill_server).
fn dispatch(req: Request, mgr: &mut SessionManager) -> (Response, bool) {
    match req {
        Request::NewSession {
            name,
            command,
            cols,
            rows,
        } => match mgr.create(name, command, cols, rows) {
            Ok(n) => (Response::Created { name: n }, false),
            Err(e) => (Response::Error { message: e }, false),
        },

        Request::KillSession { name } => match mgr.kill(&name) {
            Ok(()) => (Response::Ok, false),
            Err(e) => (Response::Error { message: e }, false),
        },

        Request::ListSessions => {
            let sessions = mgr.list();
            (Response::SessionList { sessions }, false)
        }

        Request::SendData { name, data } => match mgr.send(&name, &data) {
            Ok(()) => (Response::Ok, false),
            Err(e) => (Response::Error { message: e }, false),
        },

        Request::ViewScreen { name } => match mgr.view(&name) {
            Ok(content) => (Response::Screen { content }, false),
            Err(e) => (Response::Error { message: e }, false),
        },

        Request::ResizePty { name, cols, rows } => match mgr.resize(&name, cols, rows) {
            Ok(()) => (Response::Ok, false),
            Err(e) => (Response::Error { message: e }, false),
        },

        Request::KillServer => (Response::Ok, true),

        Request::Ping => (Response::Pong, false),
    }
}
